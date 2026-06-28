// Storage root constants. The dashboard (Understand-Anything) and our chat
// pipeline share the same on-disk format — a single knowledge-graph.json per
// repo in wiki/code_wiki/<repo>/. Keeping the file name in UA-speak ("knowledge-
// graph.json") means we can drop it straight into a future UA dashboard host
// without re-encoding.
export const RAW_CODE_ROOT = "raw/code"
export const WIKI_CODE_ROOT = "wiki/code_wiki"
export const CODEGRAPH_DIR = ".codegraph"

// Understand-Anything KnowledgeGraph type (matches
// @understand-anything/core/types KnowledgeGraph). We mirror it inline
// rather than depending on the UA package so the editor and the chat
// pipeline can read it without a build step.
//
// The dashboard reads knowledge-graph.json from GRAPH_DIR and validates
// against @understand-anything/core's zod schema. Our writes go through
// knowledge-graph-writer.ts which fills the required fields (summary,
// complexity, direction, weight, layers, tour) with sensible defaults.

// --- Node types (UA has 21; we only produce a subset) ---------------------
// Code: file / function / class / module / concept
// Non-code: config / document / service / table / endpoint / pipeline /
//           schema / resource
// Domain: domain / flow / step
// Knowledge: article / entity / topic / claim / source
export type NodeType =
  | "file" | "function" | "class" | "module" | "concept"
  | "config" | "document" | "service" | "table" | "endpoint"
  | "pipeline" | "schema" | "resource"
  | "domain" | "flow" | "step"
  | "article" | "entity" | "topic" | "claim" | "source"

export type Complexity = "simple" | "moderate" | "complex"
export type EdgeDirection = "forward" | "backward" | "bidirectional"

// Edge types (35 values across 8 categories). We only emit a handful —
// the dashboard accepts them all and we just don't fill what we don't have.
export type EdgeType =
  | "imports" | "exports" | "contains" | "inherits" | "implements"
  | "calls" | "subscribes" | "publishes" | "middleware"
  | "reads_from" | "writes_to" | "transforms" | "validates"
  | "depends_on" | "tested_by" | "configures"
  | "related" | "similar_to"
  | "deploys" | "serves" | "provisions" | "triggers"
  | "migrates" | "documents" | "routes" | "defines_schema"
  | "contains_flow" | "flow_step" | "cross_domain"
  | "cites" | "contradicts" | "builds_on" | "exemplifies"
  | "categorized_under" | "authored_by"

export interface ProjectMeta {
  name: string
  languages: string[]
  frameworks: string[]
  description: string
  analyzedAt: string
  gitCommitHash: string
}

export interface GraphNode {
  id: string
  type: NodeType
  name: string
  filePath?: string
  lineRange?: [number, number]
  summary: string
  tags: string[]
  complexity: Complexity
  languageNotes?: string
}

export interface GraphEdge {
  source: string
  target: string
  type: EdgeType
  direction: EdgeDirection
  weight: number
}

export interface Layer {
  id: string
  name: string
  description: string
  nodeIds: string[]
}

export interface TourStep {
  order: number
  title: string
  description: string
  nodeIds: string[]
  languageLesson?: string
}

// KnowledgeGraph — the canonical on-disk shape. Mirrors UA exactly so the
// dashboard can validate it. `kind` distinguishes code from wiki-knowledge
// graphs; we always set "codebase".
export interface KnowledgeGraph {
  version: string
  kind?: "codebase" | "knowledge"
  project: ProjectMeta
  nodes: GraphNode[]
  edges: GraphEdge[]
  layers: Layer[]
  tour: TourStep[]
}

// --- Our local helpers (NOT in UA) ----------------------------------------
// These describe the *result* of querying a knowledge graph for chat context.
// They are not stored on disk — only produced at retrieval time by
// graph-query.ts and consumed by chat-retrieval.ts.
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

// CodeWikiIndex — list of repos under raw/code/ with summary stats for the
// new CodeWikiView page. Not part of UA; this is our UI's index.
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

// AnalysisMeta — UA's meta.json shape. We also surface this from our own
// meta.json so the dashboard reads its theme/lastAnalyzedAt from the same
// place we track them.
export interface AnalysisMeta {
  lastAnalyzedAt: string
  gitCommitHash: string
  version: string
  analyzedFiles: number
}

// Re-exported alias so existing call-sites that import `CodeGraph` keep
// compiling. New code should use `KnowledgeGraph` directly.
export type CodeGraph = KnowledgeGraph
