import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/commands/fs", () => ({
  listDirectory: vi.fn(),
}))

import { listDirectory } from "@/commands/fs"
import { detectRepos } from "./repo-detector"
import type { FileNode } from "@/types/wiki"

let projectPath: string
beforeEach(() => {
  projectPath = "/project"
  vi.clearAllMocks()
})
afterEach(() => {
  vi.restoreAllMocks()
})

function dirNode(name: string, path: string): FileNode {
  return { name, path, is_dir: true, children: [] }
}
function fileNode(name: string, path: string): FileNode {
  return { name, path, is_dir: false, children: [] }
}

describe("detectRepos", () => {
  it("returns one repo per top-level subdir", async () => {
    vi.mocked(listDirectory).mockResolvedValue([
      dirNode("repo-A", `${projectPath}/raw/code/repo-A`),
      dirNode("repo-B", `${projectPath}/raw/code/repo-B`),
    ])
    const repos = await detectRepos(projectPath)
    expect(repos).toEqual(["repo-A", "repo-B"])
  })

  it("skips hidden directories and stray files at the top level", async () => {
    vi.mocked(listDirectory).mockResolvedValue([
      dirNode("repo-A", `${projectPath}/raw/code/repo-A`),
      dirNode(".cache", `${projectPath}/raw/code/.cache`),
      fileNode("README.md", `${projectPath}/raw/code/README.md`),
    ])
    const repos = await detectRepos(projectPath)
    expect(repos).toEqual(["repo-A"])
  })

  it("returns empty list when raw/code is missing", async () => {
    vi.mocked(listDirectory).mockRejectedValue(new Error("not found"))
    const repos = await detectRepos(projectPath)
    expect(repos).toEqual([])
  })
})