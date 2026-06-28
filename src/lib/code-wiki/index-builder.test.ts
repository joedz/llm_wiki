import { describe, expect, it, vi } from "vitest"

vi.mock("@/commands/fs", () => ({
  fileExists: vi.fn(),
  readFile: vi.fn(),
  writeFile: vi.fn(),
  createDirectory: vi.fn(),
}))

import { fileExists, readFile, writeFile, createDirectory } from "@/commands/fs"
import { writeKnowledgeGraph, writeIndex } from "./wiki-storage"
import { buildIndex } from "./index-builder"
import type { KnowledgeGraph } from "./types"

const projectPath = "/project"

function makeGraph(name: string, fileCount: number, symbolCount: number, language: string): KnowledgeGraph {
  const nodes: KnowledgeGraph["nodes"] = []
  for (let i = 0; i < fileCount; i++) {
    nodes.push({
      id: `file:${i}.ts`,
      type: "file",
      name: `${i}.ts`,
      filePath: `${i}.ts`,
      lineRange: [0, 0],
      summary: "",
      tags: [],
      complexity: "moderate",
    })
  }
  for (let i = 0; i < symbolCount; i++) {
    nodes.push({
      id: `function:${i}`,
      type: "function",
      name: `fn${i}`,
      filePath: `0.ts`,
      lineRange: [0, 0],
      summary: "",
      tags: [],
      complexity: "moderate",
    })
  }
  return {
    version: "1.0.0",
    kind: "codebase",
    project: {
      name,
      languages: [language],
      frameworks: [],
      description: "",
      analyzedAt: "2026-06-27T00:00:00Z",
      gitCommitHash: "",
    },
    nodes,
    edges: [],
    layers: [],
    tour: [],
  }
}

describe("buildIndex", () => {
  it("aggregates repo summaries from each knowledge-graph.json", async () => {
    vi.clearAllMocks()

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

    await writeKnowledgeGraph(projectPath, "repo-A", makeGraph("repo-A", 3, 12, "typescript"))
    await writeKnowledgeGraph(projectPath, "repo-B", makeGraph("repo-B", 5, 7, "rust"))
    const index = await buildIndex(projectPath, ["repo-A", "repo-B"])

    expect(index.repos).toHaveLength(2)
    expect(index.repos.map((r) => r.name).sort()).toEqual(["repo-A", "repo-B"])
    expect(index.repos[0]!.path).toBe("raw/code/repo-A")
    expect(index.repos[0]!.graphPath).toBe("wiki/code_wiki/repo-A/knowledge-graph.json")
    expect(index.version).toBe("1.0.0")
    expect(index.generatedAt).toMatch(/^\d{4}-\d{2}-\d{2}T/)
  })

  it("drops repos whose knowledge-graph.json is missing", async () => {
    vi.clearAllMocks()
    vi.mocked(fileExists).mockResolvedValue(false)
    vi.mocked(readFile).mockResolvedValue("")
    vi.mocked(writeFile).mockResolvedValue()
    vi.mocked(createDirectory).mockResolvedValue()
    const index = await buildIndex(projectPath, ["repo-A", "missing"])
    expect(index.repos).toHaveLength(0)
  })
})
