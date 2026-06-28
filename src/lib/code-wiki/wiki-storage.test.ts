import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/commands/fs", () => ({
  fileExists: vi.fn(),
  readFile: vi.fn(),
  writeFile: vi.fn(),
  createDirectory: vi.fn(),
}))

import { fileExists, readFile, writeFile, createDirectory } from "@/commands/fs"
import {
  knowledgeGraphPathFor,
  readKnowledgeGraph,
  readIndex,
  readMeta,
  repoRootFor,
  writeKnowledgeGraph,
  writeIndex,
  writeMeta,
  WIKI_CODE_ROOT,
  type AnalysisMeta,
  type CodeWikiIndex,
  type KnowledgeGraph,
} from "./index"

let root: string
beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), "code-wiki-storage-"))
  vi.clearAllMocks()
})
afterEach(() => {
  rmSync(root, { recursive: true, force: true })
})

describe("wiki-storage paths", () => {
  it("knowledgeGraphPathFor and repoRootFor produce sibling locations", () => {
    expect(knowledgeGraphPathFor(root, "repo-A")).toBe(
      `${root}/${WIKI_CODE_ROOT}/repo-A/knowledge-graph.json`,
    )
    expect(repoRootFor(root, "repo-A")).toBe(`${root}/${WIKI_CODE_ROOT}/repo-A`)
  })
})

describe("wiki-storage round-trips", () => {
  it("writes and reads knowledge-graph.json", async () => {
    const graph: KnowledgeGraph = {
      version: "1.0.0",
      kind: "codebase",
      project: {
        name: "repo-A",
        languages: ["typescript"],
        frameworks: [],
        description: "",
        analyzedAt: "2026-06-27T00:00:00Z",
        gitCommitHash: "",
      },
      nodes: [
        {
          id: "file:a.ts",
          type: "file",
          name: "a.ts",
          filePath: "a.ts",
          lineRange: [0, 0],
          summary: "File a.ts",
          tags: [],
          complexity: "moderate",
        },
      ],
      edges: [],
      layers: [],
      tour: [],
    }
    vi.mocked(fileExists).mockResolvedValue(true)
    vi.mocked(writeFile).mockResolvedValue()
    vi.mocked(createDirectory).mockResolvedValue()
    vi.mocked(readFile).mockResolvedValue(JSON.stringify(graph))
    await writeKnowledgeGraph(root, "repo-A", graph)
    const loaded = await readKnowledgeGraph(root, "repo-A")
    expect(loaded?.project.name).toBe("repo-A")
    expect(loaded?.nodes).toHaveLength(1)
  })

  it("writes and reads index.json and meta.json", async () => {
    const index: CodeWikiIndex = { version: "1.0.0", generatedAt: "2026-06-27T00:00:00Z", repos: [] }
    const meta: AnalysisMeta = {
      lastAnalyzedAt: "2026-06-27T00:00:00Z",
      gitCommitHash: "",
      version: "1.0.0",
      analyzedFiles: 0,
    }
    vi.mocked(fileExists).mockResolvedValue(true)
    vi.mocked(writeFile).mockResolvedValue()
    vi.mocked(createDirectory).mockResolvedValue()
    vi.mocked(readFile).mockResolvedValue(JSON.stringify(index))
    await writeIndex(root, index)
    await writeMeta(root, "repo-A", meta)
    expect(writeFile).toHaveBeenCalledWith(expect.stringContaining("index.json"), expect.any(String))
    expect(writeFile).toHaveBeenCalledWith(expect.stringContaining("meta.json"), expect.any(String))
  })

  it("readKnowledgeGraph returns null when missing", async () => {
    vi.mocked(fileExists).mockResolvedValue(false)
    const loaded = await readKnowledgeGraph(root, "missing")
    expect(loaded).toBeNull()
  })
})
