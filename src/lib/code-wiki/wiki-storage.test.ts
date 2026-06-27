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
  graphPathFor,
  readGraph,
  readIndex,
  readMeta,
  repoRootFor,
  writeGraph,
  writeIndex,
  writeMeta,
  WIKI_CODE_ROOT,
  type CodeGraph,
  type CodeWikiIndex,
  type CodeWikiMeta,
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
  it("graphPathFor and repoRootFor produce sibling locations", () => {
    expect(graphPathFor(root, "repo-A")).toBe(`${root}/${WIKI_CODE_ROOT}/repo-A/graph.json`)
    expect(repoRootFor(root, "repo-A")).toBe(`${root}/${WIKI_CODE_ROOT}/repo-A`)
  })
})

describe("wiki-storage round-trips", () => {
  it("writes and reads graph.json", async () => {
    const graph: CodeGraph = {
      version: "1.0.0",
      project: { name: "repo-A", languages: ["typescript"], lastAnalyzedAt: "2026-06-27T00:00:00Z", fileCount: 1, symbolCount: 1 },
      nodes: [{ id: "file:a.ts", type: "file", name: "a.ts", filePath: "a.ts", tags: [] }],
      edges: [],
      stats: { totalNodes: 1, totalEdges: 0, byLanguage: { typescript: 1 }, byNodeType: { file: 1 } },
    }
    vi.mocked(fileExists).mockResolvedValue(true)
    vi.mocked(writeFile).mockResolvedValue()
    vi.mocked(createDirectory).mockResolvedValue()
    vi.mocked(readFile).mockResolvedValue(JSON.stringify(graph))
    await writeGraph(root, "repo-A", graph)
    const loaded = await readGraph(root, "repo-A")
    expect(loaded?.project.name).toBe("repo-A")
    expect(loaded?.nodes).toHaveLength(1)
  })

  it("writes and reads index.json and meta.json", async () => {
    const index: CodeWikiIndex = { version: "1.0.0", generatedAt: "2026-06-27T00:00:00Z", repos: [] }
    const meta: CodeWikiMeta = { lastAnalyzedAt: "2026-06-27T00:00:00Z", indexerVersion: "1.0.0", sourceFileCount: 0 }
    vi.mocked(fileExists).mockResolvedValue(true)
    vi.mocked(writeFile).mockResolvedValue()
    vi.mocked(createDirectory).mockResolvedValue()
    vi.mocked(readFile).mockResolvedValue(JSON.stringify(index))
    await writeIndex(root, index)
    await writeMeta(root, "repo-A", meta)
    expect(writeFile).toHaveBeenCalledWith(expect.stringContaining("index.json"), expect.any(String))
    expect(writeFile).toHaveBeenCalledWith(expect.stringContaining("meta.json"), expect.any(String))
  })

  it("readGraph returns null when missing", async () => {
    vi.mocked(fileExists).mockResolvedValue(false)
    const loaded = await readGraph(root, "missing")
    expect(loaded).toBeNull()
  })
})