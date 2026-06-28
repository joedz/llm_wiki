import type {
  CodegraphContextEdge,
  CodegraphContextNode,
  CodegraphContextPayload,
} from "@/types/codegraph-context"
import type {
  Complexity,
  EdgeDirection,
  EdgeType,
  GraphEdge,
  GraphNode,
  KnowledgeGraph,
  NodeType,
} from "./types"

// codegraph 0.9.x node "kind" → Understand-Anything NodeType. UA has 21
// node types; we only emit the 5 code types plus `concept` (the closest
// catch-all for variables / constants / properties, which UA doesn't model
// separately). Anything we don't recognise is dropped at write time.
const NODE_TYPE_MAP: Record<string, NodeType> = {
  file: "file",
  function: "function",
  method: "function",
  class: "class",
  struct: "class",
  interface: "class",
  type_alias: "class",
  enum: "class",
  enum_member: "class",
  module: "module",
  constant: "concept",
  variable: "concept",
  property: "concept",
  // 'import' / 'component' / 'route' aren't real code-wiki nodes — we
  // either drop them (component, route) or treat as a relationship, not
  // a node. We list them here so future codegraph versions can be
  // remapped without an additional release.
  import: "module",
  component: "service",
  route: "endpoint",
}

// codegraph edge "kind" → UA EdgeType. UA has 35 edge types; we only
// emit what codegraph gives us today (contains, imports, calls).
const EDGE_TYPE_MAP: Record<string, EdgeType> = {
  contains: "contains",
  imports: "imports",
  calls: "calls",
  // The dashboard supports more, but we don't have data for them yet.
  // Adding new mappings is a one-liner when codegraph grows a new
  // relation.
}

const DEFAULT_COMPLEXITY: Complexity = "moderate"
const DEFAULT_DIRECTION: EdgeDirection = "forward"
const DEFAULT_WEIGHT = 1.0
const GRAPH_VERSION = "1.0.0"

function inferLanguageFromPath(filePath: string | undefined): string | null {
  if (!filePath) return null
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

function mapNode(node: CodegraphContextNode): GraphNode | null {
  // TS field is `type` (Rust uses #[serde(rename = "type")] on the `kind`
  // field of CodegraphContextNode).
  const uaType = NODE_TYPE_MAP[node.type]
  if (!uaType) return null

  const filePath = node.filePath || undefined
  const language = node.language ?? inferLanguageFromPath(filePath) ?? undefined
  const summary = (node.summary ?? node.docstring ?? "").trim()

  const result: GraphNode = {
    id: node.id,
    type: uaType,
    name: node.name,
    summary,
    tags: node.tags ?? [],
    complexity: DEFAULT_COMPLEXITY,
  }
  if (filePath) result.filePath = filePath
  if (node.location) {
    result.lineRange = [node.location.startLine, node.location.endLine]
  }
  if (language) result.languageNotes = language
  return result
}

function mapEdge(edge: CodegraphContextEdge): GraphEdge | null {
  // Same as above: Rust renames `kind` → `type` on the edge struct.
  const uaType = EDGE_TYPE_MAP[edge.type]
  if (!uaType) return null
  return {
    source: edge.source,
    target: edge.target,
    type: uaType,
    direction: DEFAULT_DIRECTION,
    weight: DEFAULT_WEIGHT,
  }
}

export interface WriteKnowledgeGraphInput {
  repoName: string
  source: CodegraphContextPayload
  /** Optional override; defaults to now (ISO 8601). */
  analyzedAt?: string
  /** Optional git hash; empty string if not in a git repo. */
  gitCommitHash?: string
}

/**
 * Build the canonical KnowledgeGraph that gets written to
 * `wiki/code_wiki/<repo>/knowledge-graph.json`. The shape mirrors
 * Understand-Anything's `KnowledgeGraph` so the dashboard can read it
 * directly without a conversion step.
 *
 * Filters:
 * - Drops nodes whose codegraph `kind` has no UA mapping.
 * - Drops edges whose codegraph `kind` has no UA mapping.
 * - Sorts nodes by id and edges by (source, target) for deterministic
 *   output (helps diffs and tests).
 */
export function buildKnowledgeGraph(
  input: WriteKnowledgeGraphInput,
): KnowledgeGraph {
  const nodes: GraphNode[] = []
  for (const raw of input.source.nodes) {
    const mapped = mapNode(raw)
    if (mapped) nodes.push(mapped)
  }
  nodes.sort((a, b) => a.id.localeCompare(b.id))

  const edges: GraphEdge[] = []
  for (const raw of input.source.edges) {
    const mapped = mapEdge(raw)
    if (mapped) edges.push(mapped)
  }
  edges.sort((a, b) =>
    a.source === b.source ? a.target.localeCompare(b.target) : a.source.localeCompare(b.source),
  )

  const languages = input.source.languages?.length
    ? Array.from(new Set(input.source.languages)).sort()
    : Array.from(
        new Set(
          nodes
            .map((n) => inferLanguageFromPath(n.filePath))
            .filter((l): l is string => Boolean(l)),
        ),
      ).sort()

  return {
    version: GRAPH_VERSION,
    kind: "codebase",
    project: {
      name: input.repoName,
      languages,
      frameworks: [],
      description: "",
      analyzedAt: input.analyzedAt ?? new Date().toISOString(),
      gitCommitHash: input.gitCommitHash ?? input.source.gitCommitHash ?? "",
    },
    nodes,
    edges,
    layers: [],
    tour: [],
  }
}
