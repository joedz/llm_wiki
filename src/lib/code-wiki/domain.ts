// TS client for the code-wiki **domain** pipeline.
//
// `codewiki-domain-progress` / `codewiki-domain-done` carry
// domain-pipeline-specific events. Schema mirrors `knowledge.ts`
// closely but the Rust side emits slightly different fields
// (no batch events for domain — only phase + warning + done).

import { invoke } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"

export const DOMAIN_EVENT = "codewiki-domain-progress"
export const DOMAIN_DONE_EVENT = "codewiki-domain-done"

export interface DomainRunSummary {
  pipelineId: string
  projectPath: string
  repoName: string
  finalGraphPath: string
  finalMetaPath: string
  nodeCount: number
  edgeCount: number
  kind: "domain"
  durationMs: number
  warnings: string[]
  derivedFromGraph: boolean
  usedLlm: boolean
}

export type PhaseStatus = "running" | "done" | "error"

export type DomainProgressEvent =
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
      kind: "warning"
      pipelineId: string
      phase: number
      message: string
    }

export function runDomainPipeline(
  projectPath: string,
  repoName: string,
  llm?: LlmRequestSpec,
): Promise<void> {
  return invoke("code_wiki_run_domain_pipeline", {
    projectPath,
    repoName,
    llm,
  })
}

export function getDomainGraph(
  projectPath: string,
  repoName: string,
): Promise<unknown | null> {
  return invoke("code_wiki_get_domain_graph", {
    projectPath,
    repoName,
  })
}

export function listDomainRepos(projectPath: string): Promise<string[]> {
  return invoke("code_wiki_list_domain_repos", { projectPath })
}

export interface LlmRequestSpec {
  provider: "anthropic" | "openai" | "ollama" | "custom"
  apiKey: string
  model: string
  baseUrl?: string
  maxTokens?: number
  temperature?: number
}

// ---------------------------------------------------------------------------
// P1-C: DomainGraph types (camelCase, matches Rust serde output)
// ---------------------------------------------------------------------------

/**
 * `domainMeta` is camelCase to match the Rust `DomainMeta` struct's
 * `#[serde(rename_all = "camelCase")]` annotation. The shape mirrors
 * UA's `domainMeta` exactly so a downstream consumer reading this
 * JSON sees the same fields.
 */
export interface DomainMeta {
  entities?: string[]
  businessRules?: string[]
  crossDomainInteractions?: string[]
  entryPoint?: string
  entryType?: "http" | "cli" | "event" | "cron" | "manual"
}

/**
 * A domain-graph node. `type` discriminates between the three layers:
 * `domain` (top-level), `flow` (within a domain), `step` (within a flow).
 * `filePath` is optional — domain/flow nodes typically point at a
 * directory or concept, not a single source file.
 *
 * All GraphNode fields are flattened into the top level (no `base`
 * wrapper) — this matches UA's on-disk shape.
 */
export interface DomainGraphNode {
  id: string
  type: "domain" | "flow" | "step" | string
  name: string
  filePath?: string
  summary: string
  tags?: string[]
  complexity?: "simple" | "moderate" | "complex"
  location?: { startLine: number; endLine: number }
  domainMeta?: DomainMeta
}

export interface DomainGraphEdge {
  source: string
  target: string
  type:
    | "contains_flow"
    | "flow_step"
    | "cross_domain"
    | string
  direction?: "forward" | "backward" | "bidirectional"
  weight: number
  description?: string
}

export interface DomainGraph {
  version: string
  kind: "domain"
  project: {
    name: string
    languages?: string[]
    frameworks?: string[]
    description?: string
    analyzedAt?: string
    gitCommitHash?: string
  }
  nodes: DomainGraphNode[]
  edges: DomainGraphEdge[]
  /** P0-1: whether the graph was derived from an existing
   * knowledge-graph.json (`true`) or via the lightweight scanner. */
  derivedFromGraph?: boolean
}

export function subscribeDomainProgress(
  handler: (event: DomainProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<DomainProgressEvent>(DOMAIN_EVENT, (e) => {
    handler(e.payload)
  })
}

export function subscribeDomainDone(
  handler: (summary: DomainRunSummary) => void,
): Promise<UnlistenFn> {
  return listen<DomainRunSummary>(DOMAIN_DONE_EVENT, (e) => {
    handler(e.payload)
  })
}