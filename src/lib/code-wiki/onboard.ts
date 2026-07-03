// TS client for the code-wiki onboard Tauri command.

import { invoke } from "@tauri-apps/api/core"

export interface OnboardResult {
  markdown: string
  path: string
  usedLlm: boolean
  durationMs: number
}

export interface LlmRequestSpec {
  provider: "anthropic" | "openai" | "ollama" | "custom"
  apiKey: string
  model: string
  baseUrl?: string
  maxTokens?: number
  temperature?: number
}

export function generateOnboarding(
  projectPath: string,
  repoName: string,
  llm?: LlmRequestSpec,
): Promise<OnboardResult> {
  return invoke("code_wiki_generate_onboarding", {
    projectPath,
    repoName,
    llm,
  })
}