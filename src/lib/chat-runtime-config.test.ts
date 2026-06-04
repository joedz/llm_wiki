import { describe, expect, it } from "vitest"
import { chatRuntimeConfigFromWikiState, type ChatRuntimeConfig, type ChatRuntimeState } from "./chat-runtime-config"

function makeState(overrides: Partial<ChatRuntimeState> = {}): ChatRuntimeState {
  return {
    llmConfig: {
      provider: "openai",
      apiKey: "k",
      model: "gpt-4.1-mini",
      ollamaUrl: "http://localhost:11434",
      customEndpoint: "",
      maxContextSize: 204800,
      reasoning: { mode: "auto" },
    },
    searchApiConfig: {
      provider: "none",
      apiKey: "",
      deepResearchSource: "web",
      anyTxt: { enabled: false, endpoint: "http://127.0.0.1:9920", filterDir: "", filterExt: "*", limit: 20 },
    },
    embeddingConfig: { enabled: false, endpoint: "", apiKey: "", model: "" },
    outputLanguage: "Japanese",
    dataVersion: 7,
    ...overrides,
  }
}

describe("chatRuntimeConfigFromWikiState", () => {
  it("extracts the chat pipeline runtime settings from wiki state", () => {
    const state = makeState()
    const config: ChatRuntimeConfig = chatRuntimeConfigFromWikiState(state)

    expect(config).toEqual({
      llmConfig: state.llmConfig,
      searchApiConfig: state.searchApiConfig,
      embeddingConfig: state.embeddingConfig,
      outputLanguage: "Japanese",
      dataVersion: 7,
    })
  })
})
