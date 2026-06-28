// Shared TS shape for the Rust `code_wiki_get_graph_payload` Tauri command.
// Mirrors `CodegraphContextPayload` in `src-tauri/src/commands/code_wiki.rs`
// — the Rust struct uses `#[serde(rename = "...")]` to expose camelCase
// fields here.
//
// This is the "raw" shape that codegraph's SQLite store gives us, before
// the knowledge-graph-writer fills in UA-specific defaults (summary,
// complexity, direction, weight, layers, tour).

export interface CodegraphContextNode {
  id: string
  type: string
  name: string
  filePath: string
  qualifiedName?: string
  language?: string
  summary?: string
  signature?: string
  docstring?: string
  tags?: string[]
  location?: { startLine: number; endLine: number }
  isExported?: boolean
  isAsync?: boolean
  decorators?: string[]
  visibility?: string
}

export interface CodegraphContextEdge {
  source: string
  target: string
  type: string
  weight?: number
  metadata?: Record<string, unknown>
}

export interface CodegraphContextPayload {
  projectPath?: string
  languages?: string[]
  gitCommitHash?: string
  nodes: CodegraphContextNode[]
  edges: CodegraphContextEdge[]
}
