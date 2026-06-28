import { invoke } from "@tauri-apps/api/core"
import { writeKnowledgeGraph, writeMeta } from "./wiki-storage"
import { buildIndex } from "./index-builder"
import { detectRepos } from "./repo-detector"
import { buildKnowledgeGraph } from "./knowledge-graph-writer"
import type { CodegraphContextPayload } from "@/types/codegraph-context"
import type { AnalysisMeta, KnowledgeGraph } from "./types"

const INDEXER_VERSION = "codegraph-1.0.0"

export async function buildGraphForRepo(
  projectPath: string,
  repoName: string,
): Promise<KnowledgeGraph> {
  await invoke("code_wiki_run_indexer", { projectPath, repoName })
  const payload = await invoke<CodegraphContextPayload>("code_wiki_get_graph_payload", {
    projectPath,
    repoName,
  })
  const graph = buildKnowledgeGraph({ repoName, source: payload })
  await writeKnowledgeGraph(projectPath, repoName, graph)
  const meta: AnalysisMeta = {
    lastAnalyzedAt: graph.project.analyzedAt,
    gitCommitHash: graph.project.gitCommitHash,
    version: INDEXER_VERSION,
    analyzedFiles: graph.nodes.filter((n) => n.type === "file").length,
  }
  await writeMeta(projectPath, repoName, meta)
  const repos = await detectRepos(projectPath)
  await buildIndex(projectPath, repos)
  return graph
}

export async function syncGraphForRepo(
  projectPath: string,
  repoName: string,
): Promise<KnowledgeGraph | null> {
  await invoke("code_wiki_run_sync", { projectPath, repoName })
  const payload = await invoke<CodegraphContextPayload | null>(
    "code_wiki_get_graph_payload",
    { projectPath, repoName },
  )
  if (!payload) return null
  const graph = buildKnowledgeGraph({ repoName, source: payload })
  await writeKnowledgeGraph(projectPath, repoName, graph)
  const meta: AnalysisMeta = {
    lastAnalyzedAt: graph.project.analyzedAt,
    gitCommitHash: graph.project.gitCommitHash,
    version: INDEXER_VERSION,
    analyzedFiles: graph.nodes.filter((n) => n.type === "file").length,
  }
  await writeMeta(projectPath, repoName, meta)
  const repos = await detectRepos(projectPath)
  await buildIndex(projectPath, repos)
  return graph
}
