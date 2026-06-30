import { getFileName, normalizePath } from "@/lib/path-utils"

// The chat retrieval types `CodeSnippet`, `CodeRelationship`, and
// `CodeReference` are owned by `./code-wiki/types.ts` (the source of
// truth: it has the wider unions that describe every call-site's runtime
// values, including `graph-query.ts` which assigns `reason: "match"`).
// Re-export them here so existing imports of these names through
// `./code-analysis-model` keep compiling.
import type { CodeRelationship, CodeReference, CodeSnippet } from "./code-wiki/types"
export type { CodeSnippet, CodeRelationship, CodeReference } from "./code-wiki/types"

export const RAW_CODE_ROOT = "raw/code"
export const CODE_TRUNCATION_SUFFIX = "\n[...truncated...]"

const CODE_EXTENSIONS = new Set([
  "c",
  "cc",
  "cpp",
  "cs",
  "css",
  "go",
  "h",
  "hpp",
  "html",
  "java",
  "js",
  "jsx",
  "kt",
  "lua",
  "mjs",
  "py",
  "rb",
  "rs",
  "sh",
  "sql",
  "swift",
  "ts",
  "tsx",
  "vue",
])

const QUERY_SYMBOL = /[A-Za-z_$][\w$]{2,}/g

export interface CodeAnalysisContext {
  snippets: CodeSnippet[]
  relationships: CodeRelationship[]
  references: CodeReference[]
}

export interface BuildCodeAnalysisContextInput {
  projectPath: string
  message: string
  maxContextSize?: number
}

export interface CodeSymbol {
  name: string
  filePath: string
  absPath: string
  language: string
  content: string
  startLine: number
  endLine: number
}

export interface CodeFile {
  absPath: string
  relPath: string
  language: string
  content: string
  lines: readonly string[]
  symbols: readonly CodeSymbol[]
}

export function isCodeSourceExtension(path: string): boolean {
  const name = getFileName(path)
  const ext = name.includes(".") ? name.split(".").pop()?.toLowerCase() : ""
  return Boolean(ext && CODE_EXTENSIONS.has(ext))
}

export function isCodeSourcePath(path: string): boolean {
  const normalized = normalizePath(path).toLowerCase()
  return normalized.includes(`/${RAW_CODE_ROOT}/`) || normalized.startsWith(`${RAW_CODE_ROOT}/`)
}

export function extractQuerySymbols(message: string): readonly string[] {
  const matches = message.match(QUERY_SYMBOL) ?? []
  const ignored = new Set(["what", "where", "call", "calls", "called", "function", "class", "this"])
  return [...new Set(matches.filter((word) => !ignored.has(word.toLowerCase())))]
}

export function fitCodeText(content: string, limit: number): string {
  if (content.length <= limit) return content
  return `${content.slice(0, limit - CODE_TRUNCATION_SUFFIX.length)}${CODE_TRUNCATION_SUFFIX}`
}

export function languageForCodePath(path: string): string {
  const ext = getFileName(path).split(".").pop()?.toLowerCase()
  switch (ext) {
    case "ts":
    case "tsx":
      return "TypeScript"
    case "js":
    case "jsx":
    case "mjs":
      return "JavaScript"
    case "rs":
      return "Rust"
    case "go":
      return "Go"
    case "py":
      return "Python"
    default:
      return ext ? ext.toUpperCase() : "Code"
  }
}
