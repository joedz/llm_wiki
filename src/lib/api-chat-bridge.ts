import { invoke } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"
import { runProjectChat, type ChatReference } from "@/lib/chat-pipeline"
import { chatRuntimeConfigFromWikiState } from "@/lib/chat-runtime-config"
import { useWikiStore } from "@/stores/wiki-store"

interface ApiChatBridgeRequest {
  requestId: string
  projectId: string
  projectPath: string
  projectName: string
  message: string
  useWebSearch: boolean
  useAnyTxtSearch: boolean
  stream: boolean
}

type ApiChatBridgeEvent =
  | { kind: "start" }
  | { kind: "context"; references: ChatReference[]; warnings: string[] }
  | { kind: "token"; text: string }
  | { kind: "reasoning"; text: string }
  | { kind: "done"; response: string; references: ChatReference[]; warnings: string[] }
  | { kind: "error"; error: string }

let bridgePromise: Promise<UnlistenFn> | null = null
const controllers = new Map<string, AbortController>()

async function pushEvent(requestId: string, event: ApiChatBridgeEvent): Promise<void> {
  await invoke("api_chat_bridge_push_event", { requestId, event })
}

async function handleApiChatRequest(payload: ApiChatBridgeRequest): Promise<void> {
  const config = chatRuntimeConfigFromWikiState(useWikiStore.getState())
  const controller = new AbortController()
  controllers.set(payload.requestId, controller)
  let errorHandled = false

  try {
    await runProjectChat(
      {
        projectPath: payload.projectPath,
        projectName: payload.projectName,
        message: payload.message,
        useWebSearch: payload.useWebSearch,
        useAnyTxtSearch: payload.useAnyTxtSearch,
        stream: payload.stream,
        signal: controller.signal,
        config,
      },
      {
        onStart: () => {
          void pushEvent(payload.requestId, { kind: "start" }).catch(() => {})
        },
        onContext: ({ references, warnings }) => {
          void pushEvent(payload.requestId, {
            kind: "context",
            references,
            warnings: warnings ?? [],
          }).catch(() => {})
        },
        onToken: (text) => {
          void pushEvent(payload.requestId, { kind: "token", text }).catch(() => {})
        },
        onReasoningToken: (text) => {
          void pushEvent(payload.requestId, { kind: "reasoning", text }).catch(() => {})
        },
        onDone: ({ response, references, warnings }) => {
          void pushEvent(payload.requestId, {
            kind: "done",
            response,
            references,
            warnings,
          }).catch(() => {})
        },
        onError: (error) => {
          errorHandled = true
          void pushEvent(payload.requestId, {
            kind: "error",
            error: error.message,
          }).catch(() => {})
        },
      },
    )
  } catch (error) {
    if (errorHandled) return
    void pushEvent(payload.requestId, {
      kind: "error",
      error: error instanceof Error ? error.message : String(error),
    }).catch(() => {})
  } finally {
    controllers.delete(payload.requestId)
  }
}

export function ensureApiChatBridge(): Promise<UnlistenFn> {
  if (!bridgePromise) {
    bridgePromise = (async () => {
      const unlistenRequest = await listen<ApiChatBridgeRequest>("api-chat://request", (event) => {
        void handleApiChatRequest(event.payload)
      })
      const unlistenCancel = await listen<string>("api-chat://cancel", (event) => {
        controllers.get(event.payload)?.abort()
      })
      return () => {
        unlistenCancel()
        unlistenRequest()
      }
    })()
  }
  return bridgePromise
}
