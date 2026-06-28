import { describe, expect, it } from "vitest"
import { buildKnowledgeGraph } from "./knowledge-graph-writer"
import type { CodegraphContextPayload } from "@/types/codegraph-context"

const FIXED_ANALYZED_AT = "2026-06-27T00:00:00.000Z"

function makePayload(): CodegraphContextPayload {
  return {
    languages: ["rust", "typescript"],
    gitCommitHash: "abc123",
    nodes: [
      {
        id: "file:src/main.rs",
        type: "file",
        name: "main.rs",
        filePath: "src/main.rs",
        language: "rust",
        location: { startLine: 0, endLine: 0 },
        tags: [],
      },
      {
        id: "function:src/main.rs:run",
        type: "function",
        name: "run",
        filePath: "src/main.rs",
        language: "rust",
        docstring: "Entry point",
        location: { startLine: 1, endLine: 5 },
        tags: ["public"],
      },
      {
        id: "struct:src/main.rs:Counter",
        type: "struct",
        name: "Counter",
        filePath: "src/main.rs",
        language: "rust",
        location: { startLine: 10, endLine: 20 },
        tags: [],
      },
      {
        id: "constant:src/main.rs:MAX",
        type: "constant",
        name: "MAX",
        filePath: "src/main.rs",
        language: "rust",
        location: { startLine: 0, endLine: 0 },
        tags: [],
      },
      // 'route' has no UA mapping and should be dropped.
      {
        id: "route:/users",
        type: "route",
        name: "/users",
        filePath: "src/routes.ts",
        language: "typescript",
        location: { startLine: 0, endLine: 0 },
        tags: [],
      },
    ],
    edges: [
      { source: "file:src/main.rs", target: "function:src/main.rs:run", type: "contains" },
      { source: "file:src/main.rs", target: "struct:src/main.rs:Counter", type: "contains" },
      { source: "function:src/main.rs:run", target: "struct:src/main.rs:Counter", type: "calls" },
      // 'related' isn't a codegraph edge today; we keep this to assert that
      // unknown edge types are silently dropped.
      { source: "function:src/main.rs:run", target: "constant:src/main.rs:MAX", type: "references" },
    ],
  }
}

describe("buildKnowledgeGraph", () => {
  it("emits a UA-shaped KnowledgeGraph with the right kind + version", () => {
    const graph = buildKnowledgeGraph({
      repoName: "demo",
      source: makePayload(),
      analyzedAt: FIXED_ANALYZED_AT,
    })
    expect(graph.version).toBe("1.0.0")
    expect(graph.kind).toBe("codebase")
    expect(graph.project.name).toBe("demo")
    expect(graph.project.languages).toEqual(["rust", "typescript"])
    expect(graph.project.frameworks).toEqual([])
    expect(graph.project.description).toBe("")
    expect(graph.project.analyzedAt).toBe(FIXED_ANALYZED_AT)
    expect(graph.project.gitCommitHash).toBe("abc123")
  })

  it("maps codegraph node kinds to UA node types and drops unknown ones", () => {
    const graph = buildKnowledgeGraph({
      repoName: "demo",
      source: makePayload(),
      analyzedAt: FIXED_ANALYZED_AT,
    })
    const byId = new Map(graph.nodes.map((n) => [n.id, n] as const))
    expect(byId.get("file:src/main.rs")?.type).toBe("file")
    expect(byId.get("function:src/main.rs:run")?.type).toBe("function")
    expect(byId.get("struct:src/main.rs:Counter")?.type).toBe("class")
    expect(byId.get("constant:src/main.rs:MAX")?.type).toBe("concept")
    // 'route' maps to UA's `endpoint` (closest non-code match).
    expect(byId.get("route:/users")?.type).toBe("endpoint")
  })

  it("fills required UA fields with defaults", () => {
    const graph = buildKnowledgeGraph({
      repoName: "demo",
      source: makePayload(),
      analyzedAt: FIXED_ANALYZED_AT,
    })
    for (const node of graph.nodes) {
      expect(node.summary).toBeTypeOf("string")
      expect(node.tags).toBeInstanceOf(Array)
      expect(node.complexity).toBe("moderate")
    }
  })

  it("uses docstring as summary when present", () => {
    const graph = buildKnowledgeGraph({
      repoName: "demo",
      source: makePayload(),
      analyzedAt: FIXED_ANALYZED_AT,
    })
    const run = graph.nodes.find((n) => n.id === "function:src/main.rs:run")
    expect(run?.summary).toBe("Entry point")
  })

  it("maps codegraph edges to UA edge types with defaults", () => {
    const graph = buildKnowledgeGraph({
      repoName: "demo",
      source: makePayload(),
      analyzedAt: FIXED_ANALYZED_AT,
    })
    const contains = graph.edges.find(
      (e) => e.type === "contains" && e.source === "file:src/main.rs",
    )
    expect(contains?.direction).toBe("forward")
    expect(contains?.weight).toBe(1.0)
    // 'references' has no UA mapping → dropped.
    const references = graph.edges.find(
      (e) =>
        e.source === "function:src/main.rs:run" &&
        e.target === "constant:src/main.rs:MAX",
    )
    expect(references).toBeUndefined()
  })

  it("emits empty layers and tour (Phase 2 placeholders)", () => {
    const graph = buildKnowledgeGraph({
      repoName: "demo",
      source: makePayload(),
      analyzedAt: FIXED_ANALYZED_AT,
    })
    expect(graph.layers).toEqual([])
    expect(graph.tour).toEqual([])
  })

  it("sorts nodes and edges deterministically", () => {
    const graph = buildKnowledgeGraph({
      repoName: "demo",
      source: makePayload(),
      analyzedAt: FIXED_ANALYZED_AT,
    })
    const nodeIds = graph.nodes.map((n) => n.id)
    expect(nodeIds).toEqual([...nodeIds].sort())
    const edgeKeys = graph.edges.map((e) => `${e.source}->${e.target}`)
    expect(edgeKeys).toEqual([...edgeKeys].sort())
  })
})
