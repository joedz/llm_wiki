import { listDirectory, readFile } from "@/commands/fs"
import { getFileName, getRelativePath, normalizePath } from "@/lib/path-utils"
import type { FileNode } from "@/types/wiki"
import {
  RAW_CODE_ROOT,
  extractQuerySymbols,
  fitCodeText,
  isCodeSourceExtension,
  isCodeSourcePath,
  languageForCodePath,
  type BuildCodeAnalysisContextInput,
  type CodeAnalysisContext,
  type CodeFile,
  type CodeReference,
  type CodeRelationship,
  type CodeSnippet,
  type CodeSymbol,
} from "./code-analysis-model"

export {
  RAW_CODE_ROOT,
  isCodeSourceExtension,
  isCodeSourcePath,
  type BuildCodeAnalysisContextInput,
  type CodeAnalysisContext,
  type CodeReference,
  type CodeRelationship,
  type CodeSnippet,
}

const SYMBOL_LINE = /^\s*(?:export\s+)?(?:async\s+)?(?:function|class|interface|type|const|let|var)\s+([A-Za-z_$][\w$]*)/
const CALL_WORD = /\b([A-Za-z_$][\w$]*)\s*\(/g
const CALLER_INTENT = /(who\s+calls|callers?|called\s+by|谁调用|誰調用|调用了|調用了)/i
const SUMMARY_INTENT = /(what\s+(does|is)|purpose|overview|summary|干什么|做什么|作用|用途|方案|模块|模組)/i

export async function buildCodeAnalysisContext(
  input: BuildCodeAnalysisContextInput,
): Promise<CodeAnalysisContext | null> {
  const projectPath = normalizePath(input.projectPath).replace(/\/+$/, "")
  const files = await loadCodeFiles(projectPath)
  if (files.length === 0) return null

  const querySymbols = extractQuerySymbols(input.message)
  const symbolMatches = findSymbolMatches(files, querySymbols)
  const fileMatches = symbolMatches.length === 0 ? findFileMatches(files, input.message) : []
  const overviewMatches = shouldIncludeProjectOverview(input.message, symbolMatches, fileMatches) ? findProjectOverview(files) : []
  const relationships = findCallRelationships(files, symbolMatches)
  const callerSnippets = snippetsForCallers(files, relationships)
  const snippets = limitSnippets(
    [
      ...symbolMatches.map((symbol) => toSnippet(symbol, "symbol-match")),
      ...fileMatches,
      ...overviewMatches,
      ...callerSnippets,
    ],
    input.maxContextSize,
  )

  if (snippets.length === 0 && relationships.length === 0) return null

  const references = buildCodeReferences(snippets)
  return { snippets, relationships, references }
}

async function loadCodeFiles(projectPath: string): Promise<CodeFile[]> {
  const codeRoot = `${projectPath}/${RAW_CODE_ROOT}`
  let tree: FileNode[]
  try {
    tree = await listDirectory(codeRoot)
  } catch {
    return []
  }

  const files: CodeFile[] = []
  for (const node of flattenFileNodes(tree).filter((file) => isCodeSourceExtension(file.path))) {
    try {
      const content = await readFile(node.path)
      const lines = content.split(/\r?\n/)
      const relPath = getRelativePath(node.path, projectPath)
      files.push({
        absPath: normalizePath(node.path),
        relPath,
        language: languageForCodePath(node.path),
        content,
        lines,
        symbols: extractSymbols(relPath, normalizePath(node.path), lines, languageForCodePath(node.path)),
      })
    } catch {
      continue
    }
  }
  return files
}

function flattenFileNodes(nodes: readonly FileNode[]): FileNode[] {
  const files: FileNode[] = []
  for (const node of nodes) {
    if (node.is_dir) {
      files.push(...flattenFileNodes(node.children ?? []))
    } else {
      files.push(node)
    }
  }
  return files
}

function extractSymbols(
  filePath: string,
  absPath: string,
  lines: readonly string[],
  language: string,
): CodeSymbol[] {
  return lines.flatMap((line, index) => {
    const match = SYMBOL_LINE.exec(line)
    const name = match?.[1]
    if (!name) return []
    const startLine = index + 1
    const endLine = Math.min(lines.length, startLine + 8)
    return [{
      name,
      filePath,
      absPath,
      language,
      content: lines.slice(index, endLine).join("\n"),
      startLine,
      endLine,
    }]
  })
}

function findSymbolMatches(files: readonly CodeFile[], symbols: readonly string[]): CodeSymbol[] {
  if (symbols.length === 0) return []
  return files.flatMap((file) => file.symbols.filter((symbol) => (
    symbols.some((query) => symbol.name.toLowerCase() === query.toLowerCase())
  )))
}

function findFileMatches(files: readonly CodeFile[], message: string): CodeSnippet[] {
  const tokens = extractQuerySymbols(message).map((token) => token.toLowerCase())
  if (tokens.length === 0) return []
  return files
    .filter((file) => tokens.some((token) => file.relPath.toLowerCase().includes(token)))
    .slice(0, 4)
    .map((file) => ({
      filePath: file.relPath,
      symbolName: getFileName(file.relPath),
      language: file.language,
      content: fitCodeText(file.content, 1200),
      startLine: 1,
      endLine: Math.min(file.lines.length, 40),
      reason: "file-match",
    }))
}

function shouldIncludeProjectOverview(
  message: string,
  symbolMatches: readonly CodeSymbol[],
  fileMatches: readonly CodeSnippet[],
): boolean {
  if (CALLER_INTENT.test(message) && symbolMatches.length > 0) return false
  return SUMMARY_INTENT.test(message) && symbolMatches.length + fileMatches.length === 0
}

function findProjectOverview(files: readonly CodeFile[]): CodeSnippet[] {
  return files
    .slice()
    .sort((left, right) => scoreOverviewFile(right) - scoreOverviewFile(left))
    .slice(0, 6)
    .map((file) => ({
      filePath: file.relPath,
      symbolName: getFileName(file.relPath),
      language: file.language,
      content: fitCodeText(file.content, 1200),
      startLine: 1,
      endLine: Math.min(file.lines.length, 40),
      reason: "project-overview",
    }))
}

function scoreOverviewFile(file: CodeFile): number {
  const path = file.relPath.toLowerCase()
  let score = file.symbols.length
  if (path.includes("/index.") || path.includes("/main.") || path.includes("/app.")) score += 5
  if (path.includes("/route") || path.includes("/server") || path.includes("/api")) score += 3
  return score
}

function findCallRelationships(
  files: readonly CodeFile[],
  targets: readonly CodeSymbol[],
): CodeRelationship[] {
  const targetByName = new Map(targets.map((target) => [target.name, target]))
  if (targetByName.size === 0) return []
  const relationships: CodeRelationship[] = []
  for (const file of files) {
    file.lines.forEach((line, index) => {
      for (const call of line.matchAll(CALL_WORD)) {
        const callName = call[1]
        if (!callName) continue
        const target = targetByName.get(callName)
        if (!target || target.absPath === file.absPath && line.includes(`function ${callName}`)) continue
        relationships.push({
          type: "calls",
          source: nearestSymbolName(file, index),
          target: target.name,
          sourcePath: file.relPath,
          targetPath: target.filePath,
          line: index + 1,
        })
      }
    })
  }
  return relationships.slice(0, 12)
}

function nearestSymbolName(file: CodeFile, lineIndex: number): string {
  const prior = [...file.symbols].reverse().find((symbol) => symbol.startLine <= lineIndex + 1)
  return prior?.name ?? getFileName(file.relPath)
}

function snippetsForCallers(
  files: readonly CodeFile[],
  relationships: readonly CodeRelationship[],
): CodeSnippet[] {
  return relationships.flatMap((rel) => {
    const file = files.find((candidate) => candidate.relPath === rel.sourcePath)
    const symbol = file?.symbols.find((candidate) => candidate.name === rel.source)
    return symbol ? [toSnippet(symbol, "caller")] : []
  })
}

function toSnippet(symbol: CodeSymbol, reason: CodeSnippet["reason"]): CodeSnippet {
  return {
    filePath: symbol.filePath,
    symbolName: symbol.name,
    language: symbol.language,
    content: fitCodeText(symbol.content, 1200),
    startLine: symbol.startLine,
    endLine: symbol.endLine,
    reason,
  }
}

function limitSnippets(
  snippets: readonly CodeSnippet[],
  maxContextSize: number | undefined,
): CodeSnippet[] {
  const limit = Math.max(2_000, Math.min(maxContextSize ?? 20_000, 20_000) * 0.2)
  const seen = new Set<string>()
  const kept: CodeSnippet[] = []
  let used = 0
  for (const snippet of snippets) {
    const key = `${snippet.filePath}:${snippet.symbolName}:${snippet.reason}`
    if (seen.has(key)) continue
    if (used + snippet.content.length > limit) break
    seen.add(key)
    kept.push(snippet)
    used += snippet.content.length
  }
  return kept
}

function buildCodeReferences(snippets: readonly CodeSnippet[]): CodeReference[] {
  return snippets.map((snippet) => ({
    title: snippet.symbolName,
    path: snippet.filePath,
    kind: "code",
    snippet: snippet.content.slice(0, 240),
  }))
}
