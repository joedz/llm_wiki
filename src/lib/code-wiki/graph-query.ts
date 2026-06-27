import type {
  CodeGraph,
  CodeReference,
  CodeRelationship,
  CodeSnippet,
  GraphNode,
} from "./types"

const STOP_WORDS = new Set([
  "the", "is", "at", "which", "on", "a", "an", "and", "or", "of", "to", "in",
  "for", "by", "with", "as", "what", "who", "where", "how", "why", "tell", "me",
  "show", "about", "calls", "called", "uses", "use", "find", "explain",
  "你", "我", "的", "是",
])

export interface GraphQueryInput {
  graph: CodeGraph
  message: string
  hops?: number
  maxContextSize?: number
}

export interface GraphQueryResult {
  snippets: CodeSnippet[]
  relationships: CodeRelationship[]
  references: CodeReference[]
}

function tokenize(message: string): string[] {
  return message
    .toLowerCase()
    .split(/[^\p{L}\p{N}_]+/u)
    .filter((t) => t.length >= 2 && !STOP_WORDS.has(t))
}

interface ScoredNode {
  node: GraphNode
  score: number
  matchKind: "symbol" | "file" | "path"
}

function scoreNodes(graph: CodeGraph, tokens: string[]): ScoredNode[] {
  const scored: ScoredNode[] = []
  for (const node of graph.nodes) {
    const lowerName = node.name.toLowerCase()
    const lowerPath = node.filePath.toLowerCase()
    let score = 0
    let matchKind: ScoredNode["matchKind"] | null = null
    for (const token of tokens) {
      if (lowerName === token) {
        score += 10
        matchKind = matchKind ?? "symbol"
      } else if (lowerName.includes(token)) {
        score += 5
        matchKind = matchKind ?? "symbol"
      } else if (lowerPath.endsWith(`/${token}`) || lowerPath === token) {
        score += 3
        matchKind = matchKind ?? "file"
      } else if (lowerPath.includes(token)) {
        score += 1
        matchKind = matchKind ?? "path"
      }
    }
    if (score > 0 && matchKind) {
      scored.push({ node, score, matchKind })
    }
  }
  scored.sort((a, b) => b.score - a.score)
  return scored
}

function buildAdjacency(graph: CodeGraph): {
  outgoing: Map<string, string[]>
  incoming: Map<string, string[]>
} {
  const outgoing = new Map<string, string[]>()
  const incoming = new Map<string, string[]>()
  for (const e of graph.edges) {
    if (!outgoing.has(e.source)) outgoing.set(e.source, [])
    outgoing.get(e.source)!.push(e.target)
    if (!incoming.has(e.target)) incoming.set(e.target, [])
    incoming.get(e.target)!.push(e.source)
  }
  return { outgoing, incoming }
}

function nodeToSnippet(node: GraphNode): CodeSnippet | null {
  if (node.type === "file") {
    return {
      filePath: node.filePath,
      symbolName: node.name,
      language: node.languageNotes ?? "unknown",
      content: node.content ?? "",
      startLine: node.location?.startLine ?? 0,
      endLine: node.location?.endLine ?? 0,
      reason: "match",
    }
  }
  if (!node.content) return null
  return {
    filePath: node.filePath,
    symbolName: node.name,
    language: node.languageNotes ?? "unknown",
    content: node.content,
    startLine: node.location?.startLine ?? 0,
    endLine: node.location?.endLine ?? 0,
    reason: "match",
  }
}

function trimSnippet(snippet: CodeSnippet, budget: number): CodeSnippet {
  if (snippet.content.length <= budget) return snippet
  return { ...snippet, content: snippet.content.slice(0, budget) + "\n// ...truncated" }
}

function buildRelationships(graph: CodeGraph, nodeIds: Set<string>): CodeRelationship[] {
  const relationships: CodeRelationship[] = []
  for (const edge of graph.edges) {
    if (!nodeIds.has(edge.source) || !nodeIds.has(edge.target)) continue
    const sourceNode = graph.nodes.find((n) => n.id === edge.source)
    const targetNode = graph.nodes.find((n) => n.id === edge.target)
    if (!sourceNode || !targetNode) continue
    if (edge.type !== "calls") continue
    relationships.push({
      type: "calls",
      source: sourceNode.name,
      target: targetNode.name,
      sourcePath: sourceNode.filePath,
      targetPath: targetNode.filePath,
      line: sourceNode.location?.startLine ?? 0,
    })
  }
  return relationships
}

function buildReferences(snippets: CodeSnippet[], repoName: string): CodeReference[] {
  return snippets.map((s) => ({
    title: s.symbolName,
    path: s.filePath,
    kind: "code" as const,
    source: repoName,
    snippet: s.content.slice(0, 120),
  }))
}

export function queryGraph(input: GraphQueryInput): GraphQueryResult {
  const { graph, message, hops = 1, maxContextSize = 16_000 } = input
  const tokens = tokenize(message)
  if (tokens.length === 0) return { snippets: [], relationships: [], references: [] }

  const scored = scoreNodes(graph, tokens)
  const { outgoing, incoming } = buildAdjacency(graph)

  const matched = new Set<string>()
  for (const { node } of scored) matched.add(node.id)

  for (const id of Array.from(matched)) {
    for (let hop = 0; hop < hops; hop++) {
      const next = new Set<string>()
      const currentLevel = hop === 0 ? [id] : Array.from(matched)
      for (const current of currentLevel) {
        for (const target of outgoing.get(current) ?? []) next.add(target)
        for (const source of incoming.get(current) ?? []) next.add(source)
      }
      for (const n of next) matched.add(n)
    }
  }

  const snippets: CodeSnippet[] = []
  let budget = maxContextSize
  for (const id of Array.from(matched)) {
    const node = graph.nodes.find((n) => n.id === id)
    if (!node) continue
    const snippet = nodeToSnippet(node)
    if (!snippet) continue
    if (snippet.content.length > budget) {
      if (snippets.length === 0 && budget > 20) {
        snippets.push(trimSnippet(snippet, budget))
      }
      break
    }
    budget -= snippet.content.length
    snippets.push(snippet)
    if (budget <= 0) break
  }

  const relationships = buildRelationships(graph, matched)
  const references = buildReferences(snippets, graph.project.name)
  return { snippets, relationships, references }
}
