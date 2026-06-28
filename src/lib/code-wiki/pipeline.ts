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

export function startPipeline(projectPath: string, repoName: string): Promise<void> {
  return invoke("code_wiki_run_pipeline", { projectPath, repoName })
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
