// P3-A + P4-A: TS client for the code-wiki missing-edge
// suggestions Tauri command + auto-fix command. Mirrors the
// `MissingEdgeSuggestion` and `AutoFixReport` Rust structs.

import { invoke } from "@tauri-apps/api/core"

export interface MissingEdgeSuggestion {
  ruleId: string
  nodeId: string
  filePath: string
  edgeKind: string
  suggestedTarget: string | null
  severity: "error" | "warning" | "info"
  description: string
}

export interface AutoFixNewEdge {
  source: string
  target: string
  kind: string
  direction: string
  weight: number
  description?: string
}

export interface AutoFixReport {
  edgesAdded: number
  dismissed: number
  remaining: number
  newEdges: AutoFixNewEdge[]
  notes: string[]
}

export interface LlmRequestSpec {
  provider: "anthropic" | "openai" | "ollama" | "custom"
  apiKey: string
  model: string
  baseUrl?: string
  maxTokens?: number
  temperature?: number
}

export function getMissingEdges(
  projectPath: string,
  repoName: string,
): Promise<MissingEdgeSuggestion[] | null> {
  return invoke("code_wiki_get_missing_edges", {
    projectPath,
    repoName,
  })
}

export function autoFixMissingEdges(
  projectPath: string,
  repoName: string,
  ruleIds: string[] | null,
  llm: LlmRequestSpec | null,
): Promise<AutoFixReport> {
  return invoke("code_wiki_auto_fix_missing_edges", {
    projectPath,
    repoName,
    ruleIds,
    llm,
  })
}