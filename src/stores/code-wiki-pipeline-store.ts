// Zustand store for the in-flight code-wiki pipeline. Lives at
// module scope (not per-component) so the progress panel survives
// navigation between views and the same event stream can be
// consumed by the CodeWikiView.

import { create } from "zustand"
import { subscribePipeline, type ProgressEvent } from "@/lib/code-wiki/pipeline"

interface PipelineRun {
  pipelineId: string
  repoName: string
  startedAt: number
  currentPhase: number
  currentPhaseLabel: string
  phaseStatus: "running" | "done" | "error" | "idle"
  /** Phase 3 only: progress of LLM batches. */
  batchDone: number
  batchTotal: number
  warnings: string[]
  result: "running" | "done" | "cancelled" | "error"
  summary: import("@/lib/code-wiki/pipeline").PipelineSummary | null
  /** Subscription handle — released on reset. */
  unlisten: (() => void) | null
}

interface PipelineStore {
  byProject: Record<string, PipelineRun>
  startListen: () => void
  ensureRunning: (projectPath: string) => PipelineRun | null
  begin: (projectPath: string, repoName: string) => void
  apply: (projectPath: string, event: ProgressEvent) => void
  reset: (projectPath: string) => void
}

const PIPELINE_PHASES = [
  "Pre-flight",
  "Ignore config",
  "Scan",
  "Batch",
  "Analyze (no LLM)",
  "Assemble review",
  "Architecture + tour",
  "Save",
]

export const usePipelineStore = create<PipelineStore>((set, get) => ({
  byProject: {},
  startListen: () => {
    // Subscribe once at module init. The subscription lives for the
    // app's lifetime; events for projects that aren't tracked are
    // simply ignored (currentPipelineId check).
    subscribePipeline((event) => {
      const state = get()
      // Find which project this event belongs to by pipelineId.
      const projectPath = Object.keys(state.byProject).find(
        (p) => state.byProject[p]?.pipelineId === event.pipelineId,
      )
      if (!projectPath) return
      get().apply(projectPath, event)
    }).then((unlisten) => {
      // Stash the unlisten on every entry so cleanup can fire.
      // (In practice the subscription is per-app so we don't
      // bother per-entry; the function is a no-op now.)
      void unlisten
    })
  },
  ensureRunning: (projectPath) => {
    const existing = get().byProject[projectPath]
    if (existing && existing.result === "running") return existing
    return null
  },
  begin: (projectPath, repoName) => {
    const pipelineId = `pipeline-local-${Date.now()}`
    set((s) => ({
      byProject: {
        ...s.byProject,
        [projectPath]: {
          pipelineId,
          repoName,
          startedAt: Date.now(),
          currentPhase: 0,
          currentPhaseLabel: PIPELINE_PHASES[0] ?? "Pre-flight",
          phaseStatus: "running",
          batchDone: 0,
          batchTotal: 0,
          warnings: [],
          result: "running",
          summary: null,
          unlisten: null,
        },
      },
    }))
  },
  apply: (projectPath, event) => {
    set((s) => {
      const current = s.byProject[projectPath]
      if (!current || current.pipelineId !== event.pipelineId) return s
      const next: PipelineRun = { ...current }
      switch (event.kind) {
        case "started":
          next.repoName = event.repoName
          next.currentPhase = 0
          next.currentPhaseLabel = PIPELINE_PHASES[0] ?? "Pre-flight"
          next.phaseStatus = "running"
          next.batchDone = 0
          next.batchTotal = 0
          next.warnings = []
          next.result = "running"
          next.summary = null
          break
        case "phase":
          next.currentPhase = event.phase
          next.currentPhaseLabel = event.label
          next.phaseStatus = event.status
          if (event.phase === 3) {
            // Batch phase reset; totals updated per batch event.
            if (event.status === "done") {
              next.batchDone = next.batchTotal
            }
          }
          if (event.phase === 4 && event.status === "done") {
            // After analysis completes, reset batch counters so
            // the next phase (save) doesn't show stale numbers.
            next.batchDone = 0
            next.batchTotal = 0
          }
          break
        case "batch":
          next.batchTotal = event.totalBatches
          if (event.status === "done") {
            next.batchDone = Math.max(next.batchDone, event.batchIndex + 1)
          } else if (event.status === "running") {
            next.batchDone = event.batchIndex
          }
          break
        case "warning":
          next.warnings = [...next.warnings, event.message]
          break
        case "cancelled":
          next.result = "cancelled"
          next.phaseStatus = "error"
          break
        case "done":
          next.result = event.summary.cancelled ? "cancelled" : "done"
          next.phaseStatus = "done"
          next.summary = event.summary
          break
      }
      return { byProject: { ...s.byProject, [projectPath]: next } }
    })
  },
  reset: (projectPath) => {
    set((s) => {
      const { [projectPath]: _, ...rest } = s.byProject
      return { byProject: rest }
    })
  },
}))

/** Phase labels in order (0-7). Index = phase number. */
export const PIPELINE_PHASE_LABELS = PIPELINE_PHASES
