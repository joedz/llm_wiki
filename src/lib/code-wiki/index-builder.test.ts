import { describe, expect, it, vi } from "vitest"

vi.mock("@/commands/fs", () => ({
  fileExists: vi.fn(),
  readFile: vi.fn(),
  writeFile: vi.fn(),
  createDirectory: vi.fn(),
}))

import { fileExists, readFile, writeFile, createDirectory } from "@/commands/fs"
import { writeGraph, writeIndex } from "./wiki-storage"
import { buildIndex } from "./index-builder"
import type { CodeGraph } from "./types"

const projectPath = "/project"

describe("buildIndex", () => {
  it("aggregates repo summaries from each graph.json", async () => {
    vi.clearAllMocks()

    // Track what writeGraph writes so readGraph can return it
    const storedGraphs: Record<string, string> = {}
    vi.mocked(readFile).mockImplementation((path: string) => {
      if (storedGraphs[path]) return Promise.resolve(storedGraphs[path])
      return Promise.resolve("")
    })
    vi.mocked(writeFile).mockImplementation((path: string, content: string) => {
      storedGraphs[path] = content
      return Promise.resolve()
    })
    vi.mocked(createDirectory).mockResolvedValue()
    vi.mocked(fileExists).mockImplementation((path: string) => {
      return Promise.resolve(path.includes("repo-A") || path.includes("repo-B"))
    })

    const graphA: CodeGraph = {
      version: "1.0.0",
      project: { name: "repo-A", languages: ["typescript"], lastAnalyzedAt: "2026-06-27T00:00:00Z", fileCount: 3, symbolCount: 12 },
      nodes: [], edges: [], stats: { totalNodes: 0, totalEdges: 0, byLanguage: {}, byNodeType: {} },
    }
    const graphB: CodeGraph = {
      version: "1.0.0",
      project: { name: "repo-B", languages: ["rust"], lastAnalyzedAt: "2026-06-27T00:00:00Z", fileCount: 5, symbolCount: 7 },
      nodes: [], edges: [], stats: { totalNodes: 0, totalEdges: 0, byLanguage: {}, byNodeType: {} },
    }
    await writeGraph(projectPath, "repo-A", graphA)
    await writeGraph(projectPath, "repo-B", graphB)
    const index = await buildIndex(projectPath, ["repo-A", "repo-B"])

    expect(index.repos).toHaveLength(2)
    expect(index.repos.map((r) => r.name).sort()).toEqual(["repo-A", "repo-B"])
    expect(index.repos[0]!.path).toBe("raw/code/repo-A")
    expect(index.repos[0]!.graphPath).toBe("wiki/code_wiki/repo-A/graph.json")
    expect(index.version).toBe("1.0.0")
    expect(index.generatedAt).toMatch(/^\d{4}-\d{2}-\d{2}T/)
  })

  it("drops repos whose graph.json is missing", async () => {
    vi.clearAllMocks()
    vi.mocked(fileExists).mockResolvedValue(false)
    vi.mocked(readFile).mockResolvedValue("")
    vi.mocked(writeFile).mockResolvedValue()
    vi.mocked(createDirectory).mockResolvedValue()
    const index = await buildIndex(projectPath, ["repo-A", "missing"])
    expect(index.repos).toHaveLength(0)
  })
})