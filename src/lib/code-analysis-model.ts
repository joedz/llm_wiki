import { getFileName, normalizePath } from "@/lib/path-utils"

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

export interface CodeSnippet {
  readonly filePath: string
  readonly symbolName: string
  readonly language: string
  readonly content: string
  readonly startLine: number
  readonly endLine: number
  readonly reason: "symbol-match" | "caller" | "file-match" | "project-overview"
}

export interface CodeRelationship {
  readonly type: "calls"
  readonly source: string
  readonly target: string
  readonly sourcePath: string
  readonly targetPath: string
  readonly line: number
}

export interface CodeReference {
  readonly title: string
  readonly path: string
  readonly kind: "code"
  readonly snippet: string
}

export interface CodeAnalysisContext {
  readonly snippets: readonly CodeSnippet[]
  readonly relationships: readonly CodeRelationship[]
  readonly references: readonly CodeReference[]
}

export interface BuildCodeAnalysisContextInput {
  readonly projectPath: string
  readonly message: string
  readonly maxContextSize?: number
}

export interface CodeSymbol {
  readonly name: string
  readonly filePath: string
  readonly absPath: string
  readonly language: string
  readonly content: string
  readonly startLine: number
  readonly endLine: number
}

export interface CodeFile {
  readonly absPath: string
  readonly relPath: string
  readonly language: string
  readonly content: string
  readonly lines: readonly string[]
  readonly symbols: readonly CodeSymbol[]
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
