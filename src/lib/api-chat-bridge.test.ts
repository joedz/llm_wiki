import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => {
  const listeners: Record<string, (event: { payload: unknown }) => void> = {}
  return {
    listen: vi.fn(async (event: string, cb: (event: { payload: unknown }) => void) => {
      listeners[event] = cb
      return vi.fn(() => {
        delete listeners[event]
      })
    }),
    emit: (event: string, payload: unknown) => listeners[event]?.({ payload }),
    reset: () => {
      for (const key of Object.keys(listeners)) {
        delete listeners[key]
      }
    },
    invoke: vi.fn(async () => undefined),
    runProjectChat: vi.fn(),
    chatRuntimeConfigFromWikiState: vi.fn(() => ({
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
        providerConfigs: {},
        deepResearchSource: "web",
        anyTxt: { enabled: false, endpoint: "http://127.0.0.1:9920", filterDir: "", filterExt: "*", limit: 20 },
      },
      embeddingConfig: { enabled: false, endpoint: "", apiKey: "", model: "" },
      outputLanguage: "English",
      dataVersion: 1,
    })),
  }
})

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
}))

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}))

vi.mock("./chat-pipeline", () => ({
  runProjectChat: mocks.runProjectChat,
}))

vi.mock("./chat-runtime-config", () => ({
  chatRuntimeConfigFromWikiState: mocks.chatRuntimeConfigFromWikiState,
}))

vi.mock("@/stores/wiki-store", () => ({
  useWikiStore: {
    getState: () => ({}),
  },
}))

describe("api chat bridge", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.resetModules()
    mocks.reset()
  })

  it("maps pipeline callbacks into Rust bridge events", async () => {
    mocks.runProjectChat.mockImplementation(async (_request, callbacks = {}) => {
      callbacks.onStart?.()
      callbacks.onContext?.({
        references: [{ title: "RAG", path: "wiki/concepts/rag.md", kind: "wiki" }],
        warnings: [],
      })
      callbacks.onToken?.("RAG")
      callbacks.onReasoningToken?.("thinking")
      callbacks.onDone?.({
        response: "RAG [1]",
        references: [{ title: "RAG", path: "wiki/concepts/rag.md", kind: "wiki" }],
        warnings: [],
      })
      return {
        response: "RAG [1]",
        references: [{ title: "RAG", path: "wiki/concepts/rag.md", kind: "wiki" }],
        warnings: [],
      }
    })

    const { ensureApiChatBridge } = await import("./api-chat-bridge")
    await ensureApiChatBridge()

    mocks.emit("api-chat://request", {
      requestId: "req-1",
      projectId: "project-1",
      projectPath: "/tmp/project",
      projectName: "Demo",
      message: "What is RAG?",
      useWebSearch: false,
      useAnyTxtSearch: false,
      stream: true,
    })
    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(mocks.invoke.mock.calls).toEqual([
      ["api_chat_bridge_push_event", { requestId: "req-1", event: { kind: "start" } }],
      ["api_chat_bridge_push_event", {
        requestId: "req-1",
        event: {
          kind: "context",
          references: [{ title: "RAG", path: "wiki/concepts/rag.md", kind: "wiki" }],
          warnings: [],
        },
      }],
      ["api_chat_bridge_push_event", { requestId: "req-1", event: { kind: "token", text: "RAG" } }],
      ["api_chat_bridge_push_event", { requestId: "req-1", event: { kind: "reasoning", text: "thinking" } }],
      ["api_chat_bridge_push_event", {
        requestId: "req-1",
        event: {
          kind: "done",
          response: "RAG [1]",
          references: [{ title: "RAG", path: "wiki/concepts/rag.md", kind: "wiki" }],
          warnings: [],
        },
      }],
    ])
  })

  it("aborts the in-flight pipeline when Rust sends cancel", async () => {
    let capturedSignal: AbortSignal | undefined

    mocks.runProjectChat.mockImplementation(async (request) => {
      capturedSignal = request.signal
      await new Promise((_, reject) => {
        request.signal?.addEventListener("abort", () => {
          const error = new Error("aborted")
          error.name = "AbortError"
          reject(error)
        }, { once: true })
      })
      return {
        response: "",
        references: [],
        warnings: [],
      }
    })

    const { ensureApiChatBridge } = await import("./api-chat-bridge")
    await ensureApiChatBridge()

    mocks.emit("api-chat://request", {
      requestId: "req-cancel",
      projectId: "project-1",
      projectPath: "/tmp/project",
      projectName: "Demo",
      message: "Stop me",
      useWebSearch: false,
      useAnyTxtSearch: false,
      stream: true,
    })
    await new Promise((resolve) => setTimeout(resolve, 0))
    mocks.emit("api-chat://cancel", "req-cancel")
    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(capturedSignal?.aborted).toBe(true)
    expect(mocks.invoke).toHaveBeenCalledWith("api_chat_bridge_push_event", {
      requestId: "req-cancel",
      event: { kind: "error", error: "aborted" },
    })
  })
})
