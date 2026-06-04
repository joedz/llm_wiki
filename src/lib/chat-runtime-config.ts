import type { EmbeddingConfig, LlmConfig, OutputLanguage, SearchApiConfig } from "@/stores/wiki-store"

export interface ChatRuntimeState {
  llmConfig: LlmConfig
  searchApiConfig: SearchApiConfig
  embeddingConfig: EmbeddingConfig
  outputLanguage: OutputLanguage
  dataVersion: number
}

export interface ChatRuntimeConfig extends ChatRuntimeState {}

export function chatRuntimeConfigFromWikiState(state: ChatRuntimeState): ChatRuntimeConfig {
  return { ...state }
}
