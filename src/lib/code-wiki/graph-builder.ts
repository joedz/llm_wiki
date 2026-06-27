import { invoke } from "@tauri-apps/api/core"
import { writeGraph, writeMeta } from "./wiki-storage"
import { buildIndex } from "./index-builder"
import { detectRepos } from "./repo-detector"
import { exportGraph, type CodegraphPayload } from "./graph-exporter"
import type { CodeGraph, CodeWikiMeta } from "./types"

const INDEXER_VERSION = "codegraph-1.0.0"

export async function buildGraphForRepo(
  projectPath: string,
  repoName: string,
): Promise<CodeGraph> {
  await invoke("code_wiki_run_indexer", { projectPath, repoName })
  const payload = await invoke<CodegraphPayload>("code_wiki_get_graph_payload", {
    projectPath,
    repoName,
  })
  const graph = exportGraph({ repoName, source: payload })
  await writeGraph(projectPath, repoName, graph)
  const meta: CodeWikiMeta = {
    lastAnalyzedAt: graph.project.lastAnalyzedAt,
    gitCommitHash: graph.project.gitCommitHash,
    indexerVersion: INDEXER_VERSION,
    sourceFileCount: graph.project.fileCount,
  }
  await writeMeta(projectPath, repoName, meta)
  const repos = await detectRepos(projectPath)
  await buildIndex(projectPath, repos)
  return graph
}

export async function syncGraphForRepo(
  projectPath: string,
  repoName: string,
): Promise<CodeGraph | null> {
  await invoke("code_wiki_run_sync", { projectPath, repoName })
  const payload = await invoke<CodegraphPayload | null>("code_wiki_get_graph_payload", {
    projectPath,
    repoName,
  })
  if (!payload) return null
  const graph = exportGraph({ repoName, source: payload })
  await writeGraph(projectPath, repoName, graph)
  const meta: CodeWikiMeta = {
    lastAnalyzedAt: graph.project.lastAnalyzedAt,
    gitCommitHash: graph.project.gitCommitHash,
    indexerVersion: INDEXER_VERSION,
    sourceFileCount: graph.project.fileCount,
  }
  await writeMeta(projectPath, repoName, meta)
  const repos = await detectRepos(projectPath)
  await buildIndex(projectPath, repos)
  return graph
}
