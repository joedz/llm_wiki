// TS client for the code-wiki 7-phase pipeline. The pipeline
// runs as a Rust background task; the TS side kicks it off,
// subscribes to the `codewiki-pipeline-progress` event stream,
// and surfaces progress to the React UI.

import { invoke } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"

export const PIPELINE_EVENT = "codewiki-pipeline-progress"

export interface PipelineSummary {
  pipelineId: string
  projectPath: string
  repoName: string
  finalGraphPath: string
  finalMetaPath: string
  finalFingerprintsPath: string
  nodeCount: number
  edgeCount: number
  layerCount: number
  tourStepCount: number
  durationMs: number
  cancelled: boolean
  warnings: string[]
  /**
   * Optional LLM `--review` verdict (approved/issues/warnings/
   * narrative) — populated when the pipeline ran Phase 8.5
   * with the `reviewLl` parameter. The dashboard surface
   * uses `narrative` for a one-line summary.
   */
  reviewNarrative?: {
    approved: boolean
    issues: string[]
    warnings: string[]
    narrative: string
  } | null
  /**
   * Optional Phase 5.5 LLM assemble-reviewer report
   * (`types_remapped`, `complexity_remapped`,
   * `cross_batch_edges_added`, `notes`). Populated when the
   * pipeline ran with `assembleReviewLl`.
   */
  assembleReview?: {
    fixedSectionOk: boolean
    nodesRecovered: number
    edgesRestored: number
    crossBatchEdgesAdded: number
    typesRemapped: number
    complexityRemapped: number
    notes: string[]
  } | null
}

export type PhaseStatus = "running" | "done" | "error"

export type ProgressEvent =
  | { kind: "started"; pipelineId: string; repoName: string; totalPhases: number }
  | { kind: "phase"; pipelineId: string; phase: number; label: string; status: PhaseStatus }
  | {
      kind: "batch"
      pipelineId: string
      phase: number
      batchIndex: number
      totalBatches: number
      fileCount: number
      status: PhaseStatus
    }
  | { kind: "warning"; pipelineId: string; phase: number; message: string }
  | { kind: "cancelled"; pipelineId: string; phase: number; partialSaved: boolean }
  | { kind: "done"; pipelineId: string; summary: PipelineSummary }

export function startPipeline(
  projectPath: string,
  repoName: string,
  llm?: LlmRequestSpec,
  options?: {
    reviewLl?: LlmRequestSpec
    /** Run Phase 5.5 LLM assemble-reviewer after deterministic
     *  assemble; useful for cleaning up unknown node kinds /
     *  complexities / cross-batch imports. */
    assembleReviewLl?: LlmRequestSpec
  },
): Promise<void> {
  return invoke("code_wiki_run_pipeline", {
    projectPath,
    repoName,
    llm,
    reviewLl: options?.reviewLl,
    assembleReviewLl: options?.assembleReviewLl,
  })
}

/**
 * Subset of `LlmConfig` that the code-wiki pipeline needs to
 * make an HTTP call. The Rust side accepts this and routes the
 * request to the right provider (Anthropic, OpenAI, Ollama,
 * custom). We don't pass the full LlmConfig because the chat
 * panel's other fields (apiMode, reasoning, etc.) are irrelevant
 * to the code-wiki batch calls.
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
 * Convert a full `LlmConfig` (the chat panel's shape, from
 * `useWikiStore.llmConfig`) into the pipeline's `LlmRequestSpec`.
 * Returns `null` if the config is missing the API key or model
 * — the caller treats that as "no LLM available" and falls back
 * to the codegraph-only path.
 */
export function llmSpecFromConfig(
  cfg: { provider?: string; apiKey?: string; model?: string; ollamaUrl?: string; customEndpoint?: string } | null | undefined,
): LlmRequestSpec | null {
  if (!cfg) return null
  const provider = (cfg.provider ?? "").toLowerCase()
  if (!cfg.apiKey && provider !== "ollama") return null
  if (!cfg.model) return null
  const mapped: LlmRequestSpec["provider"] =
    provider === "anthropic" ? "anthropic"
    : provider === "ollama" ? "ollama"
    : provider === "custom" ? "custom"
    : "openai"
  const spec: LlmRequestSpec = {
    provider: mapped,
    apiKey: cfg.apiKey ?? "",
    model: cfg.model,
  }
  if (mapped === "ollama" && cfg.ollamaUrl) spec.baseUrl = cfg.ollamaUrl
  else if (mapped === "custom" && cfg.customEndpoint) spec.baseUrl = cfg.customEndpoint
  return spec
}

/** Convenience: does the current LlmConfig look usable? */
export function hasLlmConfig(
  cfg: { apiKey?: string; model?: string; provider?: string; ollamaUrl?: string; customEndpoint?: string } | null | undefined,
): boolean {
  return llmSpecFromConfig(cfg) !== null
}

export function cancelPipeline(pipelineId: string): Promise<boolean> {
  return invoke("code_wiki_cancel_pipeline", { pipelineId })
}

/**
 * Subscribe to pipeline progress events. Returns the unlisten
 * function — call it on component unmount to stop receiving events.
 */
export function subscribePipeline(
  handler: (event: ProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<ProgressEvent>(PIPELINE_EVENT, (e) => {
    handler(e.payload)
  })
}
