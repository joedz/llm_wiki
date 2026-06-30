// Real-filesystem demo: writes a synthetic but well-formed UA
// `KnowledgeGraph` + `meta.json` + `index.json` into a temp project
// using `fs` directly (the official TS `write*` helpers funnel
// through `@tauri-apps/api`'s `invoke`, which is only available
// inside the Tauri webview). The on-disk layout mirrors what the
// app writes when a user clicks "Build code graph" in CodeWikiView.
//
// Round-trip validation confirms:
//   - `wiki-storage.*`'s path helpers produce the correct file paths
//   - the JSON shape on disk matches the `KnowledgeGraph` contract
//   - the dashboard would accept the layout (`kind: "codebase"`,
//     `project.frameworks` / `description` / `gitCommitHash`,
//     non-empty `layers[]` + `tour[]`)

import { describe, expect, it } from "vitest"
import { mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, dirname } from "node:path"

import {
  knowledgeGraphPathFor,
  metaPathFor,
  indexPathFor,
  repoRootFor,
} from "./wiki-storage"
import type { AnalysisMeta, CodeWikiIndex, KnowledgeGraph } from "./types"

function emptyGraph(name: string): KnowledgeGraph {
  return {
    version: "1.0.0",
    kind: "codebase",
    project: {
      name,
      languages: ["typescript", "rust"],
      frameworks: ["Tauri", "React", "Vite", "Vitest"],
      description: "A small demo project used to exercise the code-wiki writer.",
      analyzedAt: "2026-07-01T00:00:00.000Z",
      gitCommitHash: "demo0001",
    },
    nodes: [
      { id: "file:src/main.ts", type: "file", name: "main.ts", filePath: "src/main.ts", summary: "Entry point", tags: ["entry"], complexity: "simple", lineRange: [1, 12] },
      { id: "file:src/lib.ts", type: "file", name: "lib.ts", filePath: "src/lib.ts", summary: "Library helpers", tags: ["util"], complexity: "simple", lineRange: [1, 40] },
      { id: "function:src/lib.ts:hello", type: "function", name: "hello", filePath: "src/lib.ts", summary: "Says hi", tags: ["greet"], complexity: "simple", lineRange: [3, 5] },
    ],
    edges: [
      { source: "file:src/main.ts", target: "file:src/lib.ts", type: "imports", direction: "forward", weight: 1 },
      { source: "function:src/lib.ts:hello", target: "function:src/lib.ts:hello", type: "calls", direction: "forward", weight: 1 },
    ],
    layers: [
      { id: "entry", name: "Entry", description: "Application boot", nodeIds: ["file:src/main.ts"] },
      { id: "lib", name: "Library", description: "Reusable lib code", nodeIds: ["file:src/lib.ts", "function:src/lib.ts:hello"] },
    ],
    tour: [
      { order: 1, title: "Project entry point", description: "Start here.", nodeIds: ["file:src/main.ts"] },
      { order: 2, title: "Core library", description: "Reusable helpers.", nodeIds: ["file:src/lib.ts"] },
    ],
  }
}

function writeJsonAtomic(filePath: string, value: unknown): void {
  mkdirSync(dirname(filePath), { recursive: true })
  writeFileSync(filePath, JSON.stringify(value, null, 2), "utf-8")
}

describe("code-wiki real-fs demo", () => {
  it("produces the same on-disk layout the app would write", () => {
    const project = mkdtempSync(join(tmpdir(), "codewiki-demo-"))
    const repoName = "demo"
    try {
      const graph = emptyGraph(repoName)
      const meta: AnalysisMeta = {
        lastAnalyzedAt: graph.project.analyzedAt,
        gitCommitHash: graph.project.gitCommitHash,
        version: "codewiki-1.0.0",
        analyzedFiles: graph.nodes.filter((n) => n.type === "file").length,
      }
      const index: CodeWikiIndex = {
        version: "1.0.0",
        generatedAt: "2026-07-01T00:00:00.000Z",
        repos: [
          {
            name: repoName,
            path: `raw/code/${repoName}`,
            graphPath: knowledgeGraphPathFor(project, repoName),
            languages: graph.project.languages,
            fileCount: graph.nodes.filter((n) => n.type === "file").length,
            symbolCount: graph.nodes.filter((n) => n.type !== "file").length,
            description: graph.project.description,
            lastAnalyzedAt: meta.lastAnalyzedAt,
          },
        ],
      }

      const graphPath = knowledgeGraphPathFor(project, repoName)
      const metaPath = metaPathFor(project, repoName)
      const indexPath = indexPathFor(project)
      const repoDir = repoRootFor(project, repoName)

      expect(graphPath.endsWith(`${repoName}/knowledge-graph.json`)).toBe(true)
      expect(metaPath.endsWith(`${repoName}/meta.json`)).toBe(true)
      expect(indexPath.endsWith("wiki/code_wiki/index.json")).toBe(true)

      writeJsonAtomic(graphPath, graph)
      writeJsonAtomic(metaPath, meta)
      writeJsonAtomic(indexPath, index)

      // Read back
      const onDiskGraph = JSON.parse(readFileSync(graphPath, "utf-8")) as KnowledgeGraph
      const onDiskMeta = JSON.parse(readFileSync(metaPath, "utf-8")) as AnalysisMeta
      const onDiskIndex = JSON.parse(readFileSync(indexPath, "utf-8")) as CodeWikiIndex

      expect(onDiskGraph.kind).toBe("codebase")
      expect(onDiskGraph.project.name).toBe("demo")
      expect(onDiskGraph.project.frameworks).toEqual(
        expect.arrayContaining(["Tauri", "React", "Vite", "Vitest"]),
      )
      expect(onDiskGraph.project.description).toMatch(/small demo project/i)
      expect(onDiskGraph.tour[0]?.title).toBe("Project entry point")
      expect(onDiskGraph.layers).toHaveLength(2)
      expect(onDiskMeta.gitCommitHash).toBe("demo0001")
      expect(onDiskMeta.version).toBe("codewiki-1.0.0")
      expect(onDiskIndex.version).toBe("1.0.0")
      expect(onDiskIndex.repos[0]?.name).toBe("demo")

      // eslint-disable-next-line no-console
      console.log("\n[code-wiki demo] repo dir:", repoDir)
      // eslint-disable-next-line no-console
      console.log(
        "[code-wiki demo] knowledge-graph.json preview:\n" +
          JSON.stringify(
            {
              version: onDiskGraph.version,
              kind: onDiskGraph.kind,
              project: onDiskGraph.project,
              nodeCount: onDiskGraph.nodes.length,
              edgeCount: onDiskGraph.edges.length,
              layerCount: onDiskGraph.layers.length,
              tourStepCount: onDiskGraph.tour.length,
              sampleNode: onDiskGraph.nodes[0],
              sampleEdge: onDiskGraph.edges[0],
              sampleLayer: onDiskGraph.layers[0],
              sampleTour: onDiskGraph.tour[0],
            },
            null,
            2,
          ),
      )
      // eslint-disable-next-line no-console
      console.log("[code-wiki demo] meta.json:\n" + JSON.stringify(onDiskMeta, null, 2))
      // eslint-disable-next-line no-console
      console.log("[code-wiki demo] index.json:\n" + JSON.stringify(onDiskIndex, null, 2))
    } finally {
      rmSync(project, { recursive: true, force: true })
    }
  })
})
