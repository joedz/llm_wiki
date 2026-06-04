import { beforeEach, describe, expect, it, vi } from "vitest"
import type { ChatRuntimeConfig } from "./chat-runtime-config"

const { streamHarness, mockBuildChatRetrievalContext } = vi.hoisted(() => {
  const pending: Array<{
    callbacks: {
      onToken: (token: string) => void
      onReasoningToken?: (token: string) => void
      onDone: () => void
      onError: (error: Error) => void
    }
    messages: Array<{ role: string; content: string }>
    signal: AbortSignal | undefined
    aborted: boolean
    settled: boolean
    resolve: () => void
  }> = []

  const mock = vi.fn(
    async (
      _config,
      messages: Array<{ role: string; content: string }>,
      callbacks: {
        onToken: (token: string) => void
        onReasoningToken?: (token: string) => void
        onDone: () => void
        onError: (error: Error) => void
      },
      signal?: AbortSignal,
    ) => {
      let resolve = () => {}
      const promise = new Promise<void>((innerResolve) => {
        resolve = innerResolve
      })

      const entry = {
        callbacks,
        messages,
        signal,
        aborted: false,
        settled: false,
        resolve: () => {
          if (entry.settled) return
          entry.settled = true
          resolve()
        },
      }

      pending.push(entry)

      if (signal) {
        if (signal.aborted) {
          entry.aborted = true
          callbacks.onDone()
          entry.resolve()
        } else {
          signal.addEventListener("abort", () => {
            if (entry.settled) return
            entry.aborted = true
            callbacks.onDone()
            entry.resolve()
          })
        }
      }

      await promise
    },
  )

  return {
    streamHarness: {
      mock,
      reset() {
        pending.length = 0
        mock.mockClear()
      },
      latest() {
        return pending[pending.length - 1]
      },
      async complete(response: string, index?: number) {
        const call = pending[index ?? pending.length - 1]
        if (!call || call.settled) return
        call.callbacks.onToken(response)
        call.callbacks.onDone()
        call.resolve()
        await Promise.resolve()
      },
      async fail(error: Error, index?: number) {
        const call = pending[index ?? pending.length - 1]
        if (!call || call.settled) return
        call.callbacks.onError(error)
        call.resolve()
        await Promise.resolve()
      },
      anyAborted() {
        return pending.some((call) => call.aborted)
      },
    },
    mockBuildChatRetrievalContext: vi.fn(),
  }
})

vi.mock("@/lib/llm-client", async () => {
  const actual = await vi.importActual<typeof import("@/lib/llm-client")>("@/lib/llm-client")
  return {
    ...actual,
    streamChat: streamHarness.mock,
  }
})

vi.mock("./chat-retrieval", () => ({
  buildChatRetrievalContext: mockBuildChatRetrievalContext,
}))

import { runProjectChat } from "./chat-pipeline"

function makeConfig(overrides: Partial<ChatRuntimeConfig> = {}): ChatRuntimeConfig {
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
      providerConfigs: {},
      deepResearchSource: "web",
      anyTxt: {
        enabled: false,
        endpoint: "http://127.0.0.1:9920",
        filterDir: "",
        filterExt: "*",
        limit: 20,
      },
    },
    embeddingConfig: { enabled: false, endpoint: "", apiKey: "", model: "" },
    outputLanguage: "English",
    dataVersion: 1,
    ...overrides,
  }
}

function makeRetrievalContext() {
  return {
    purpose: "Answer accurately.",
    index: "## Concepts\n- [[RAG]]",
    wikiPages: [
      {
        id: "rag",
        title: "RAG",
        path: "wiki/concepts/rag.md",
        content: "# RAG\n\nRetrieval augmented generation.",
        priority: 0,
      },
    ],
    externalResults: [],
    references: [
      {
        title: "RAG",
        path: "wiki/concepts/rag.md",
        kind: "wiki" as const,
        snippet: "Retrieval augmented generation.",
      },
    ],
    warnings: [],
  }
}

function createDeferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((innerResolve, innerReject) => {
    resolve = innerResolve
    reject = innerReject
  })
  return { promise, resolve, reject }
}

async function waitForStreamStart(): Promise<void> {
  await Promise.resolve()
  await Promise.resolve()
}

describe("runProjectChat", () => {
  beforeEach(() => {
    streamHarness.reset()
    mockBuildChatRetrievalContext.mockReset()
    mockBuildChatRetrievalContext.mockResolvedValue(makeRetrievalContext())
  })

  it("aggregates streamed tokens into a non-streaming result with references and warnings", async () => {
    const promise = runProjectChat({
      projectPath: "/tmp/project",
      projectName: "Demo",
      message: "What is RAG?",
      useWebSearch: false,
      useAnyTxtSearch: false,
      stream: false,
      config: makeConfig(),
    })

    await waitForStreamStart()
    await streamHarness.complete("RAG stands for retrieval augmented generation [1].")

    await expect(promise).resolves.toEqual({
      response: "RAG stands for retrieval augmented generation [1].",
      references: [
        expect.objectContaining({ path: "wiki/concepts/rag.md", kind: "wiki" }),
      ],
      warnings: [],
    })
  })

  it("emits start, context, reasoning, token, and done callbacks in streaming mode", async () => {
    const events: string[] = []
    const promise = runProjectChat(
      {
        projectPath: "/tmp/project",
        projectName: "Demo",
        message: "What is RAG?",
        useWebSearch: false,
        useAnyTxtSearch: false,
        stream: true,
        config: makeConfig(),
      },
      {
        onStart: () => events.push("start"),
        onContext: () => events.push("context"),
        onReasoningToken: () => events.push("reasoning"),
        onToken: () => events.push("token"),
        onDone: () => events.push("done"),
      },
    )

    await waitForStreamStart()
    const pending = streamHarness.latest()
    pending?.callbacks.onReasoningToken?.("Let me think")
    await streamHarness.complete("RAG stands for retrieval augmented generation [1].")

    await promise

    expect(events).toEqual(["start", "context", "reasoning", "token", "done"])
  })

  it("passes prior history into the prompt and injects the language reminder on the final user turn", async () => {
    const promise = runProjectChat({
      projectPath: "/tmp/project",
      projectName: "Demo",
      message: "What is RAG?",
      history: [
        { role: "user", content: "Earlier question" },
        { role: "assistant", content: "Earlier answer [1]." },
      ],
      useWebSearch: false,
      useAnyTxtSearch: false,
      stream: false,
      config: makeConfig({ outputLanguage: "English" }),
    })

    await waitForStreamStart()
    const call = streamHarness.latest()

    expect(call?.messages[0]?.role).toBe("system")
    expect(call?.messages[1]).toEqual({ role: "user", content: "Earlier question" })
    expect(call?.messages[2]).toEqual({ role: "assistant", content: "Earlier answer [1]." })
    expect(call?.messages[3]?.role).toBe("user")
    expect(call?.messages[3]?.content).toContain("REMINDER: All output must be in English.")
    expect(call?.messages[3]?.content).toContain("What is RAG?")

    await streamHarness.complete("RAG [1]")
    await promise
  })

  it("short-circuits retrieval for greeting-only messages", async () => {
    const promise = runProjectChat({
      projectPath: "/tmp/project",
      projectName: "Demo",
      message: "hello",
      useWebSearch: false,
      useAnyTxtSearch: false,
      stream: false,
      config: makeConfig(),
    })

    await waitForStreamStart()
    expect(mockBuildChatRetrievalContext).not.toHaveBeenCalled()
    expect(streamHarness.latest()?.messages[0]?.content).toContain("casual greeting")

    await streamHarness.complete("Hello there!")

    await expect(promise).resolves.toEqual({
      response: "Hello there!",
      references: [],
      warnings: [],
    })
  })

  it("forwards the abort signal and resolves with partial output when cancelled", async () => {
    const controller = new AbortController()
    const events: string[] = []

    const promise = runProjectChat(
      {
        projectPath: "/tmp/project",
        projectName: "Demo",
        message: "What is RAG?",
        useWebSearch: false,
        useAnyTxtSearch: false,
        stream: true,
        signal: controller.signal,
        config: makeConfig(),
      },
      {
        onDone: () => events.push("done"),
      },
    )

    await waitForStreamStart()
    const pending = streamHarness.latest()
    pending?.callbacks.onToken("Partial")
    controller.abort()

    await expect(promise).resolves.toEqual({
      response: "Partial",
      references: [
        expect.objectContaining({ path: "wiki/concepts/rag.md", kind: "wiki" }),
      ],
      warnings: [],
    })
    expect(events).toEqual(["done"])
    expect(streamHarness.anyAborted()).toBe(true)
  })

  it("stops cleanly when aborted while retrieval is still pending", async () => {
    const controller = new AbortController()
    const retrieval = createDeferred<ReturnType<typeof makeRetrievalContext>>()
    const onContext = vi.fn()
    const onDone = vi.fn()
    mockBuildChatRetrievalContext.mockReturnValueOnce(retrieval.promise)

    const promise = runProjectChat(
      {
        projectPath: "/tmp/project",
        projectName: "Demo",
        message: "What is RAG?",
        useWebSearch: false,
        useAnyTxtSearch: false,
        stream: true,
        signal: controller.signal,
        config: makeConfig(),
      },
      { onContext, onDone },
    )

    controller.abort()
    retrieval.resolve(makeRetrievalContext())

    await expect(promise).rejects.toMatchObject({ name: "AbortError" })
    expect(onContext).not.toHaveBeenCalled()
    expect(onDone).not.toHaveBeenCalled()
    expect(streamHarness.mock).not.toHaveBeenCalled()
  })

  it("does not enter streamChat when cancellation lands before streaming begins", async () => {
    const controller = new AbortController()
    const onContext = vi.fn()
    const onDone = vi.fn()
    mockBuildChatRetrievalContext.mockImplementationOnce(async () => {
      controller.abort()
      return makeRetrievalContext()
    })

    const promise = runProjectChat(
      {
        projectPath: "/tmp/project",
        projectName: "Demo",
        message: "What is RAG?",
        useWebSearch: false,
        useAnyTxtSearch: false,
        stream: true,
        signal: controller.signal,
        config: makeConfig(),
      },
      { onContext, onDone },
    )

    await expect(promise).rejects.toMatchObject({ name: "AbortError" })
    expect(onContext).not.toHaveBeenCalled()
    expect(onDone).not.toHaveBeenCalled()
    expect(streamHarness.mock).not.toHaveBeenCalled()
  })

  it("bubbles retrieval failures before stream startup without calling streamChat", async () => {
    const failure = new Error("retrieval exploded")
    mockBuildChatRetrievalContext.mockRejectedValueOnce(failure)

    const promise = runProjectChat({
      projectPath: "/tmp/project",
      projectName: "Demo",
      message: "What is RAG?",
      useWebSearch: false,
      useAnyTxtSearch: false,
      stream: true,
      config: makeConfig(),
    })

    await expect(promise).rejects.toThrow("retrieval exploded")
    expect(streamHarness.mock).not.toHaveBeenCalled()
  })

  it("rejects the run when the LLM stream errors", async () => {
    const onError = vi.fn()
    const promise = runProjectChat(
      {
        projectPath: "/tmp/project",
        projectName: "Demo",
        message: "What is RAG?",
        useWebSearch: false,
        useAnyTxtSearch: false,
        stream: false,
        config: makeConfig(),
      },
      { onError },
    )

    await waitForStreamStart()
    const failure = new Error("boom")
    await streamHarness.fail(failure)

    await expect(promise).rejects.toThrow("boom")
    expect(onError).toHaveBeenCalledWith(failure)
  })
})
