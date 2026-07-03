// TS client for the code-wiki **knowledge** pipeline.
//
// Mirrors `src/lib/code-wiki/pipeline.ts` (the codebase pipeline),
// but for `codewiki-knowledge-progress` / `codewiki-knowledge-done`.
// Same shape contract: emit camelCase fields on the wire (Rust
// pipeline emits serde-rename'd camelCase via `#[serde(rename_all =
// "camelCase")]`).

import { invoke } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"

export const KNOWLEDGE_EVENT = "codewiki-knowledge-progress"
export const KNOWLEDGE_DONE_EVENT = "codewiki-knowledge-done"

export interface KnowledgeStats {
  articles: number
  claims: number
  entities: number
  topics: number
  sources: number
  edges: number
  wikilinksUnresolved: number
  llmFailures: number
}

export interface KnowledgeRunSummary {
  pipelineId: string
  projectPath: string
  repoName: string
  finalGraphPath: string
  finalMetaPath: string
  nodeCount: number
  edgeCount: number
  kind: "knowledge"
  durationMs: number
  warnings: string[]
  stats: KnowledgeStats
}

export type PhaseStatus = "running" | "done" | "error"

export type KnowledgeProgressEvent =
  | {
      kind: "started"
      pipelineId: string
      repoName: string
      totalPhases: number
    }
  | {
      kind: "phase"
      pipelineId: string
      phase: number
      label: string
      status: PhaseStatus
    }
  | {
      kind: "batch"
      pipelineId: string
      phase: number
      batchIndex: number
      totalBatches: number
      fileCount: number
      status: PhaseStatus
    }

export function runKnowledgePipeline(
  projectPath: string,
  repoName: string,
  llm?: LlmRequestSpec,
): Promise<void> {
  return invoke("code_wiki_run_knowledge_pipeline", {
    projectPath,
    repoName,
    llm,
  })
}

export function getKnowledgeGraph(
  projectPath: string,
  repoName: string,
): Promise<unknown | null> {
  return invoke("code_wiki_get_knowledge_graph", {
    projectPath,
    repoName,
  })
}

export function listKnowledgeRepos(projectPath: string): Promise<string[]> {
  return invoke("code_wiki_list_knowledge_repos", { projectPath })
}

/**
 * Mirror of `LlmRequestSpec` from `pipeline.ts`. Duplicated here
 * because the knowledge pipeline is independent of the codebase
 * pipeline and we don't want TS re-exports to drag in the
 * codebase-only `startPipeline` helper.
 */
export interface LlmRequestSpec {
  provider: "anthropic" | "openai" | "ollama" | "custom"
  apiKey: string
  model: string
  baseUrl?: string
  maxTokens?: number
  temperature?: number
}

/**
 * Subscribe to knowledge-pipeline progress events. Returns the
 * unlisten function — call it on component unmount to stop
 * receiving events.
 */
export function subscribeKnowledgeProgress(
  handler: (event: KnowledgeProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<KnowledgeProgressEvent>(KNOWLEDGE_EVENT, (e) => {
    handler(e.payload)
  })
}

export function subscribeKnowledgeDone(
  handler: (summary: KnowledgeRunSummary) => void,
): Promise<UnlistenFn> {
  return listen<KnowledgeRunSummary>(KNOWLEDGE_DONE_EVENT, (e) => {
    handler(e.payload)
  })
}