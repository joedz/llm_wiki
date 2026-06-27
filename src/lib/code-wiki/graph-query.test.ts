import { describe, expect, it } from "vitest"
import { queryGraph } from "./graph-query"
import type { CodeGraph } from "./types"
import sample from "./__fixtures__/sample-graph.json" assert { type: "json" }

const graph = sample as CodeGraph

describe("queryGraph", () => {
  it("matches a symbol by exact name and includes its callers", () => {
    const result = queryGraph({ graph, message: "who calls alpha", hops: 1 })
    expect(result.snippets.map((s) => s.symbolName)).toEqual(
      expect.arrayContaining(["alpha", "beta", "gamma"]),
    )
    expect(result.references[0]!.kind).toBe("code")
  })

  it("matches a file by name and includes the file's contents", () => {
    const result = queryGraph({ graph, message: "show me b.ts", hops: 0 })
    expect(result.snippets.some((s) => s.filePath === "src/b.ts")).toBe(true)
  })

  it("returns empty result for unrelated message", () => {
    const result = queryGraph({ graph, message: "tell me about elephants", hops: 1 })
    expect(result.snippets).toHaveLength(0)
    expect(result.relationships).toHaveLength(0)
  })

  it("respects the context budget", () => {
    const result = queryGraph({ graph, message: "alpha", hops: 1, maxContextSize: 60 })
    const total = result.snippets.reduce((sum, s) => sum + s.content.length, 0)
    expect(total).toBeLessThanOrEqual(60)
  })
})
