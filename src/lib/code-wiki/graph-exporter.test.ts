import { describe, expect, it } from "vitest"
import { exportGraph } from "./graph-exporter"
import type { CodeGraph } from "./types"

describe("exportGraph", () => {
  it("normalises a codegraph-style node to our schema", () => {
    const result = exportGraph({
      repoName: "demo",
      source: {
        languages: ["typescript"],
        gitCommitHash: "abc123",
        nodes: [
          {
            id: "file:src/foo.ts",
            type: "file",
            name: "foo.ts",
            filePath: "src/foo.ts",
            tags: ["source"],
            location: { startLine: 0, endLine: 10 },
          },
        ],
        edges: [],
      },
    })
    expect(result.project.name).toBe("demo")
    expect(result.project.languages).toEqual(["typescript"])
    expect(result.project.gitCommitHash).toBe("abc123")
    expect(result.nodes).toHaveLength(1)
    expect(result.nodes[0]!.type).toBe("file")
    expect(result.stats.totalNodes).toBe(1)
    expect(result.stats.byLanguage.typescript).toBe(1)
  })

  it("maps method/let/const/var to function/variable", () => {
    const result = exportGraph({
      repoName: "demo",
      source: {
        languages: ["typescript"],
        nodes: [
          { id: "a", type: "method", name: "m", filePath: "a.ts", tags: [] },
          { id: "b", type: "const", name: "C", filePath: "a.ts", tags: [] },
          { id: "c", type: "let", name: "l", filePath: "a.ts", tags: [] },
          { id: "d", type: "var", name: "v", filePath: "a.ts", tags: [] },
        ],
        edges: [],
      },
    })
    expect(result.nodes.map((n) => n.type)).toEqual(["function", "variable", "variable", "variable"])
  })

  it("filters out unknown edge types and maps known ones", () => {
    const result = exportGraph({
      repoName: "demo",
      source: {
        languages: ["typescript"],
        nodes: [
          { id: "a", type: "function", name: "a", filePath: "a.ts", tags: [] },
          { id: "b", type: "function", name: "b", filePath: "b.ts", tags: [] },
        ],
        edges: [
          { source: "a", target: "b", type: "calls" },
          { source: "a", target: "b", type: "unknown_type" },
        ],
      },
    })
    expect(result.edges).toHaveLength(1)
    expect(result.edges[0]!.type).toBe("calls")
  })

  it("computes fileCount and symbolCount correctly", () => {
    const result = exportGraph({
      repoName: "demo",
      source: {
        languages: ["typescript"],
        nodes: [
          { id: "f1", type: "file", name: "a.ts", filePath: "a.ts", tags: [] },
          { id: "f2", type: "file", name: "b.ts", filePath: "b.ts", tags: [] },
          { id: "s1", type: "function", name: "x", filePath: "a.ts", tags: [] },
          { id: "s2", type: "class", name: "Y", filePath: "b.ts", tags: [] },
        ],
        edges: [],
      },
    })
    expect(result.project.fileCount).toBe(2)
    expect(result.project.symbolCount).toBe(2)
  })
})
