import { streamChat, type ChatMessage as LLMMessage } from "@/lib/llm-client"
import { detectLanguage } from "@/lib/detect-language"
import { isGreeting } from "@/lib/greeting-detector"
import { getLanguagePromptName } from "@/lib/language-metadata"
import { buildChatPromptMessages } from "./chat-prompt-builder"
import { buildChatRetrievalContext, type ChatReference } from "./chat-retrieval"
import type { ChatRuntimeConfig } from "./chat-runtime-config"

export type { ChatReference } from "./chat-retrieval"

export interface ChatRunRequest {
  projectPath: string
  projectName: string
  message: string
  history?: LLMMessage[]
  useWebSearch: boolean
  useAnyTxtSearch: boolean
  stream: boolean
  signal?: AbortSignal
  config: ChatRuntimeConfig
}

export interface ChatRunResult {
  response: string
  references: ChatReference[]
  warnings: string[]
}

export interface ChatRunCallbacks {
  onStart?: () => void
  onContext?: (payload: { references: ChatReference[]; warnings: string[] }) => void
  onToken?: (text: string) => void
  onReasoningToken?: (text: string) => void
  onDone?: (result: ChatRunResult) => void
  onError?: (error: Error) => void
}

function createAbortError(): Error {
  const error = new Error("Request aborted")
  error.name = "AbortError"
  return error
}

function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) {
    throw createAbortError()
  }
}

function resolveOutputLanguage(config: ChatRuntimeConfig, fallbackText: string): string {
  if (config.outputLanguage && config.outputLanguage !== "auto") {
    return config.outputLanguage
  }
  return detectLanguage(fallbackText || "English")
}

function buildLanguageReminder(outputLanguage: string): string {
  return `REMINDER: All output must be in ${getLanguagePromptName(outputLanguage)}. Do not use any other language.`
}

function injectReminderIntoFinalUser(messages: LLMMessage[], reminder: string): LLMMessage[] {
  if (!reminder || messages.length === 0) return messages

  const lastIndex = messages.length - 1
  const lastMessage = messages[lastIndex]

  if (!lastMessage || lastMessage.role !== "user") return messages

  return [
    ...messages.slice(0, lastIndex),
    { ...lastMessage, content: `[${reminder}]\n\n${lastMessage.content}` },
  ]
}

function buildGreetingMessages(
  request: ChatRunRequest,
  outputLanguage: string,
): LLMMessage[] {
  return [
    {
      role: "system",
      content: [
        `You are a wiki assistant for the project "${request.projectName}".`,
        "The user sent a casual greeting. Reply briefly and naturally, in one or two sentences.",
        "Do NOT invent wiki content or pretend to have retrieved pages. Invite the user to ask a concrete question if they want information from the wiki.",
        "",
        `Respond in ${outputLanguage}.`,
      ].join("\n"),
    },
    ...(request.history ?? []),
    {
      role: "user",
      content: request.message,
    },
  ]
}

export async function runProjectChat(
  request: ChatRunRequest,
  callbacks: ChatRunCallbacks = {},
): Promise<ChatRunResult> {
  throwIfAborted(request.signal)
  callbacks.onStart?.()

  const hasProjectContext = Boolean(request.projectPath && request.projectName)
  const greetingOnly = isGreeting(request.message)
  const outputLanguage = resolveOutputLanguage(request.config, request.message)

  const retrieval = !hasProjectContext || greetingOnly
    ? {
        purpose: "",
        index: "",
        wikiPages: [],
        externalResults: [],
        references: [],
        warnings: [],
      }
    : await buildChatRetrievalContext({
        projectPath: request.projectPath,
        projectName: request.projectName,
        message: request.message,
        useWebSearch: request.useWebSearch,
        useAnyTxtSearch: request.useAnyTxtSearch,
        config: request.config,
      })

  throwIfAborted(request.signal)
  callbacks.onContext?.({
    references: retrieval.references,
    warnings: retrieval.warnings,
  })

  const baseMessages = !hasProjectContext
    ? [
        ...(request.history ?? []),
        {
          role: "user" as const,
          content: request.message,
        },
      ]
    : greetingOnly
    ? buildGreetingMessages(request, outputLanguage)
    : buildChatPromptMessages({
        projectName: request.projectName,
        message: request.message,
        history: request.history ?? [],
        outputLanguage,
        retrieval,
      })

  const messages = !hasProjectContext || greetingOnly
    ? baseMessages
    : injectReminderIntoFinalUser(baseMessages, buildLanguageReminder(outputLanguage))

  let response = ""
  let streamError: Error | null = null
  let streamStarted = false

  await streamChat(
    request.config.llmConfig,
    messages,
    {
      onToken: (token) => {
        streamStarted = true
        response += token
        callbacks.onToken?.(token)
      },
      onReasoningToken: (token) => {
        streamStarted = true
        callbacks.onReasoningToken?.(token)
      },
      onDone: () => {},
      onError: (error) => {
        streamError = error
        callbacks.onError?.(error)
      },
    },
    request.signal,
  )

  if (streamError) {
    throw streamError
  }

  if (request.signal?.aborted && !streamStarted) {
    throw createAbortError()
  }

  const result: ChatRunResult = {
    response,
    references: retrieval.references,
    warnings: retrieval.warnings,
  }

  callbacks.onDone?.(result)
  return result
}
