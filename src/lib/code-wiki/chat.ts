// TS client for the code-wiki chat Tauri command.

import { invoke } from "@tauri-apps/api/core"

export interface ChatMessage {
  role: "user" | "assistant"
  content: string
}

export interface ChatResult {
  answer: string
  primaryNodeIds: string[]
  secondaryNodeIds: string[]
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

export function chatQuery(
  projectPath: string,
  repoName: string,
  query: string,
  history: ChatMessage[],
  llm?: LlmRequestSpec,
): Promise<ChatResult> {
  return invoke("code_wiki_chat_query", {
    projectPath,
    repoName,
    query,
    history,
    llm,
  })
}