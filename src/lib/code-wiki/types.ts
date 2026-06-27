export const RAW_CODE_ROOT = "raw/code"
export const WIKI_CODE_ROOT = "wiki/code_wiki"
export const CODEGRAPH_DIR = ".codegraph"

export type NodeType =
  | "file"
  | "function"
  | "class"
  | "interface"
  | "type"
  | "module"
  | "variable"

export type EdgeType =
  | "imports"
  | "contains"
  | "calls"
  | "extends"
  | "implements"
  | "defines"
  | "references"

export interface GraphNode {
  id: string
  type: NodeType
  name: string
  filePath: string
  summary?: string
  tags: string[]
  complexity?: "simple" | "moderate" | "complex"
  languageNotes?: string
  location?: { startLine: number; endLine: number }
  signature?: string
  content?: string
}

export interface GraphEdge {
  source: string
  target: string
  type: EdgeType
  weight?: number
  metadata?: Record<string, unknown>
}

export interface CodeGraph {
  version: "1.0.0"
  project: {
    name: string
    description?: string
    languages: string[]
    lastAnalyzedAt: string
    gitCommitHash?: string
    fileCount: number
    symbolCount: number
  }
  nodes: GraphNode[]
  edges: GraphEdge[]
  layers?: Array<{ id: string; name: string; description: string; nodeIds: string[] }>
  stats: {
    totalNodes: number
    totalEdges: number
    byLanguage: Record<string, number>
    byNodeType: Record<string, number>
  }
}

export interface RepoSummary {
  name: string
  path: string
  graphPath: string
  languages: string[]
  fileCount: number
  symbolCount: number
  description?: string
  lastAnalyzedAt: string
}

export interface CodeWikiIndex {
  version: "1.0.0"
  generatedAt: string
  repos: RepoSummary[]
}

export interface CodeWikiMeta {
  lastAnalyzedAt: string
  gitCommitHash?: string
  indexerVersion: string
  sourceFileCount: number
}

export interface CodeSnippet {
  filePath: string
  symbolName: string
  language: string
  content: string
  startLine: number
  endLine: number
  reason: string
}

export interface CodeRelationship {
  type: "calls" | "imports" | "contains" | "extends" | "implements"
  source: string
  target: string
  sourcePath: string
  targetPath: string
  line: number
}

export interface CodeReference {
  title: string
  path: string
  kind: "code"
  source?: string
  snippet?: string
}