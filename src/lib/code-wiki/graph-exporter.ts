import type { CodeGraph, EdgeType, GraphEdge, GraphNode, NodeType } from "./types"

interface CodegraphNode {
  id: string
  type: string
  name: string
  filePath: string
  tags?: string[]
  complexity?: string
  summary?: string
  location?: { startLine: number; endLine: number }
  signature?: string
  content?: string
}

interface CodegraphEdge {
  source: string
  target: string
  type: string
  weight?: number
  metadata?: Record<string, unknown>
}

export interface CodegraphPayload {
  languages: string[]
  gitCommitHash?: string
  nodes: CodegraphNode[]
  edges: CodegraphEdge[]
}

const NODE_TYPE_MAP: Record<string, NodeType> = {
  file: "file",
  function: "function",
  method: "function",
  class: "class",
  interface: "interface",
  type: "type",
  module: "module",
  variable: "variable",
  const: "variable",
  let: "variable",
  var: "variable",
}

const EDGE_TYPE_MAP: Record<string, EdgeType> = {
  imports: "imports",
  contains: "contains",
  calls: "calls",
  extends: "extends",
  implements: "implements",
  defines: "defines",
  references: "references",
}

export interface ExportInput {
  repoName: string
  source: CodegraphPayload
}

function inferLanguageFromPath(filePath: string): string | null {
  const ext = filePath.split(".").pop()?.toLowerCase()
  if (!ext) return null
  const map: Record<string, string> = {
    ts: "typescript",
    tsx: "typescript",
    js: "javascript",
    jsx: "javascript",
    rs: "rust",
    py: "python",
    go: "go",
    rb: "ruby",
    java: "java",
    cs: "csharp",
    cpp: "cpp",
    c: "c",
    h: "c",
    hpp: "cpp",
    swift: "swift",
    kt: "kotlin",
    php: "php",
    sh: "shell",
    md: "markdown",
  }
  return map[ext] ?? null
}

export function exportGraph(input: ExportInput): CodeGraph {
  const nodes: GraphNode[] = input.source.nodes.map((n) => ({
    id: n.id,
    type: NODE_TYPE_MAP[n.type] ?? "module",
    name: n.name,
    filePath: n.filePath,
    summary: n.summary,
    tags: n.tags ?? [],
    complexity: (n.complexity as GraphNode["complexity"]) ?? undefined,
    location: n.location,
    signature: n.signature,
    content: n.content,
  }))

  const edges: GraphEdge[] = input.source.edges
    .filter((e) => EDGE_TYPE_MAP[e.type])
    .map((e) => ({
      source: e.source,
      target: e.target,
      type: EDGE_TYPE_MAP[e.type]!,
      weight: e.weight,
      metadata: e.metadata,
    }))

  const fileCount = nodes.filter((n) => n.type === "file").length
  const symbolCount = nodes.length - fileCount

  const byLanguage: Record<string, number> = {}
  for (const node of nodes) {
    const lang = inferLanguageFromPath(node.filePath) ?? "unknown"
    byLanguage[lang] = (byLanguage[lang] ?? 0) + 1
  }

  const byNodeType: Record<string, number> = {}
  for (const node of nodes) {
    byNodeType[node.type] = (byNodeType[node.type] ?? 0) + 1
  }

  return {
    version: "1.0.0",
    project: {
      name: input.repoName,
      languages: input.source.languages,
      lastAnalyzedAt: new Date().toISOString(),
      gitCommitHash: input.source.gitCommitHash,
      fileCount,
      symbolCount,
    },
    nodes,
    edges,
    stats: { totalNodes: nodes.length, totalEdges: edges.length, byLanguage, byNodeType },
  }
}
