// Chat panel for the code-wiki graph.
//
// Sidebar-style panel with a message list and an input box. The
// user types a question about the codebase, hits Enter, and we
// call `code_wiki_chat_query` (synchronous — v1 non-streaming).
//
// The user's chat history is kept in component state (capped at
// 10 messages) and re-sent with each query so the LLM has
// conversational context.

import { useState } from "react"
import { useTranslation } from "react-i18next"
import { Loader2, MessageCircle, Send, X } from "lucide-react"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { Button } from "@/components/ui/button"
import { chatQuery, type ChatMessage, type ChatResult } from "@/lib/code-wiki/chat"
import { llmSpecFromConfig } from "@/lib/code-wiki/pipeline"
import { useWikiStore } from "@/stores/wiki-store"
import { normalizePath } from "@/lib/path-utils"

interface Props {
  open: boolean
  projectPath: string
  repoName: string
  onClose: () => void
}

const MAX_HISTORY = 10

export function ChatPanel({ open, projectPath, repoName, onClose }: Props) {
  const { t } = useTranslation()
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [input, setInput] = useState("")
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [lastResult, setLastResult] = useState<ChatResult | null>(null)

  if (!open) return null

  const send = async () => {
    const q = input.trim()
    if (!q) return
    setInput("")
    setError(null)
    const userMsg: ChatMessage = { role: "user", content: q }
    const nextHistory = [...messages, userMsg].slice(-MAX_HISTORY)
    setMessages(nextHistory)
    setSending(true)
    try {
      const llmConfig = useWikiStore.getState().llmConfig
      const llm = llmSpecFromConfig(llmConfig)
      const result = await chatQuery(
        normalizePath(projectPath),
        repoName,
        q,
        nextHistory.slice(0, -1), // exclude the message we just added
        llm ?? undefined,
      )
      setLastResult(result)
      setMessages((prev) => [...prev, { role: "assistant", content: result.answer }])
    } catch (err) {
      setError(String(err))
    } finally {
      setSending(false)
    }
  }

  return (
    <div
      data-testid="chat-panel"
      className="fixed inset-y-0 right-0 z-40 flex w-[420px] max-w-[90vw] flex-col border-l bg-card shadow-xl"
    >
      <header className="flex items-center justify-between border-b p-3">
        <div className="flex items-center gap-2">
          <MessageCircle className="h-4 w-4" />
          <h3 className="text-sm font-semibold">
            {t("codeWiki.chat.title", "Chat")}
            <span className="ml-2 text-xs font-normal text-muted-foreground">
              {repoName}
            </span>
          </h3>
        </div>
        <Button variant="ghost" size="icon" onClick={onClose}>
          <X className="h-4 w-4" />
        </Button>
      </header>

      <div className="flex-1 space-y-3 overflow-auto p-3 text-sm">
        {messages.length === 0 && (
          <p className="text-muted-foreground">
            {t(
              "codeWiki.chat.empty",
              "Ask anything about this codebase. Examples: 'where is auth handled?', 'which functions call db.query?'",
            )}
          </p>
        )}
        {messages.map((m, i) => (
          <div
            key={i}
            className={`rounded-md p-2 ${
              m.role === "user"
                ? "ml-6 bg-primary/10"
                : "mr-6 bg-muted"
            }`}
          >
            <div className="mb-1 text-xs font-semibold uppercase text-muted-foreground">
              {m.role === "user" ? t("codeWiki.chat.you", "You") : t("codeWiki.chat.assistant", "Assistant")}
            </div>
            {m.role === "assistant" ? (
              <div className="prose prose-sm max-w-none dark:prose-invert">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{m.content}</ReactMarkdown>
              </div>
            ) : (
              <p className="whitespace-pre-wrap">{m.content}</p>
            )}
          </div>
        ))}
        {sending && (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Loader2 className="h-3 w-3 animate-spin" />
            {t("codeWiki.chat.thinking", "Thinking…")}
          </div>
        )}
        {error && (
          <div className="rounded border border-destructive/50 bg-destructive/10 p-2 text-destructive">
            {error}
          </div>
        )}
        {lastResult && !sending && (
          <div className="text-xs text-muted-foreground">
            {lastResult.usedLlm ? "LLM" : "retrieval only"} ·{" "}
            {lastResult.primaryNodeIds.length} primary ·{" "}
            {lastResult.secondaryNodeIds.length} secondary · {lastResult.durationMs}ms
          </div>
        )}
      </div>

      <div className="border-t p-3">
        <div className="flex items-center gap-2">
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !sending) {
                e.preventDefault()
                void send()
              }
            }}
            placeholder={t("codeWiki.chat.placeholder", "Ask about the codebase…")}
            disabled={sending}
            className="flex-1 rounded border bg-background px-2 py-1 text-sm"
          />
          <Button onClick={send} disabled={sending || !input.trim()} size="icon">
            <Send className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>
    </div>
  )
}