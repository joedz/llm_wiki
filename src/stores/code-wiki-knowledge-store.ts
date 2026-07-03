// Zustand store for the in-flight code-wiki knowledge pipeline.
//
// Mirrors `code-wiki-pipeline-store.ts` but for the knowledge
// pipeline. We keep the two stores separate so that the codebase
// pipeline progress and knowledge pipeline progress can both run
// at the same time without one overwriting the other.

import { create } from "zustand"

import {
  subscribeKnowledgeProgress,
  subscribeKnowledgeDone,
  type KnowledgeProgressEvent,
  type KnowledgeRunSummary,
} from "@/lib/code-wiki/knowledge"

interface KnowledgeRun {
  pipelineId: string
  repoName: string
  startedAt: number
  currentPhase: number
  currentPhaseLabel: string
  phaseStatus: "running" | "done" | "error" | "idle"
  /** Phase 1 only: LLM article-analyzer batch progress. */
  batchDone: number
  batchTotal: number
  warnings: string[]
  result: "running" | "done" | "cancelled" | "error"
  summary: KnowledgeRunSummary | null
  unlisten: (() => void) | null
}

interface KnowledgeStore {
  byProject: Record<string, KnowledgeRun>
  startListen: () => void
  ensureRunning: (projectPath: string) => KnowledgeRun | null
  begin: (projectPath: string, repoName: string) => void
  apply: (projectPath: string, event: KnowledgeProgressEvent) => void
  setDone: (projectPath: string, summary: KnowledgeRunSummary) => void
  reset: (projectPath: string) => void
}

const KNOWLEDGE_PHASES = [
  "Scan + parse",   // 0
  "Enrich (LLM)",   // 1
  "Save",           // 2
]

export const useKnowledgeStore = create<KnowledgeStore>((set, get) => ({
  byProject: {},
  startListen: () => {
    let progressUnlisten: (() => void) | null = null
    let doneUnlisten: (() => void) | null = null

    subscribeKnowledgeProgress((event) => {
      const state = get()
      const projectPath = Object.keys(state.byProject).find(
        (p) => state.byProject[p]?.pipelineId === event.pipelineId,
      )
      if (!projectPath) return
      get().apply(projectPath, event)
    }).then((u) => {
      progressUnlisten = u
    })
    subscribeKnowledgeDone((summary) => {
      const state = get()
      const projectPath = Object.keys(state.byProject).find(
        (p) => state.byProject[p]?.pipelineId === summary.pipelineId,
      )
      if (!projectPath) return
      get().setDone(projectPath, summary)
    }).then((u) => {
      doneUnlisten = u
    })

    // Cleanup on app teardown — best-effort since zustand has no
    // global destructor.
    void progressUnlisten
    void doneUnlisten
  },
  ensureRunning: (projectPath) => {
    const existing = get().byProject[projectPath]
    if (existing && existing.result === "running") return existing
    return null
  },
  begin: (projectPath, repoName) => {
    const pipelineId = projectPath
    set((s) => ({
      byProject: {
        ...s.byProject,
        [projectPath]: {
          pipelineId,
          repoName,
          startedAt: Date.now(),
          currentPhase: 0,
          currentPhaseLabel: KNOWLEDGE_PHASES[0] ?? "Scan + parse",
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
      const isStarted = event.kind === "started"
      if (!current && !isStarted) return s
      if (!isStarted && current && current.pipelineId !== event.pipelineId) return s
      const next: KnowledgeRun = current
        ? { ...current }
        : {
            pipelineId: event.kind === "started" ? event.pipelineId : "",
            repoName: event.kind === "started" ? event.repoName : (current?.repoName ?? ""),
            startedAt: Date.now(),
            currentPhase: 0,
            currentPhaseLabel: KNOWLEDGE_PHASES[0] ?? "Scan + parse",
            phaseStatus: "running" as const,
            batchDone: 0,
            batchTotal: 0,
            warnings: [],
            result: "running" as const,
            summary: null,
            unlisten: null,
          }
      switch (event.kind) {
        case "started":
          next.pipelineId = event.pipelineId
          next.repoName = event.repoName
          next.currentPhase = 0
          next.currentPhaseLabel = KNOWLEDGE_PHASES[0] ?? "Scan + parse"
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
          if (event.phase === 1 && event.status === "done") {
            // After LLM enrich phase completes, reset batch counters.
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
      }
      return { byProject: { ...s.byProject, [projectPath]: next } }
    })
  },
  setDone: (projectPath, summary) => {
    set((s) => {
      const current = s.byProject[projectPath]
      if (!current) return s
      return {
        byProject: {
          ...s.byProject,
          [projectPath]: {
            ...current,
            result: "done",
            phaseStatus: "done",
            summary,
            warnings: [...current.warnings, ...summary.warnings],
          },
        },
      }
    })
  },
  reset: (projectPath) => {
    set((s) => {
      const { [projectPath]: _, ...rest } = s.byProject
      return { byProject: rest }
    })
  },
}))

export const KNOWLEDGE_PHASE_LABELS = KNOWLEDGE_PHASES