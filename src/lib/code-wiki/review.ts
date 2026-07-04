// P3-A: TS client for the code-wiki missing-edge suggestions Tauri
// command. Mirrors the `MissingEdgeSuggestion` Rust struct.

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

export function getMissingEdges(
  projectPath: string,
  repoName: string,
): Promise<MissingEdgeSuggestion[] | null> {
  return invoke("code_wiki_get_missing_edges", {
    projectPath,
    repoName,
  })
}