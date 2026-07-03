// Onboard modal — show the 6-section onboarding markdown.
//
// Two-step flow:
//   1. User clicks the Onboard button (only enabled when built).
//   2. We invoke `code_wiki_generate_onboarding` (which writes
//      onboarding.md to the wiki dir AND returns the markdown).
//   3. Render the markdown via react-markdown.
//
// Optional follow-up actions: Copy to clipboard / Save to
// docs/ONBOARDING.md in the source tree (a second Tauri call).

import { useState } from "react"
import { useTranslation } from "react-i18next"
import { Loader2, BookOpen, X, Copy, Save } from "lucide-react"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { Button } from "@/components/ui/button"
import { generateOnboarding, type OnboardResult } from "@/lib/code-wiki/onboard"
import { llmSpecFromConfig } from "@/lib/code-wiki/pipeline"
import { useWikiStore } from "@/stores/wiki-store"
import { normalizePath } from "@/lib/path-utils"
import { invoke } from "@tauri-apps/api/core"

interface Props {
  open: boolean
  projectPath: string
  repoName: string
  onClose: () => void
}

export function OnboardModal({ open, projectPath, repoName, onClose }: Props) {
  const { t } = useTranslation()
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<OnboardResult | null>(null)
  const [copied, setCopied] = useState(false)

  const run = async () => {
    setLoading(true)
    setError(null)
    try {
      const llmConfig = useWikiStore.getState().llmConfig
      const llm = llmSpecFromConfig(llmConfig)
      const r = await generateOnboarding(
        normalizePath(projectPath),
        repoName,
        llm ?? undefined,
      )
      setResult(r)
    } catch (err) {
      setError(String(err))
    } finally {
      setLoading(false)
    }
  }

  const copyMarkdown = async () => {
    if (!result) return
    try {
      await navigator.clipboard.writeText(result.markdown)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      /* ignore */
    }
  }

  const saveToDocs = async () => {
    if (!result) return
    try {
      const path = await invoke<string>("code_wiki_save_onboarding", {
        projectPath: normalizePath(projectPath),
        repoName,
        markdown: result.markdown,
      })
      // Surface the saved path briefly
      setError(`Saved to ${path}`)
      setTimeout(() => setError(null), 3000)
    } catch (err) {
      setError(String(err))
    }
  }

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-4"
      role="dialog"
      aria-modal="true"
      data-testid="onboard-modal"
    >
      <div className="flex max-h-[85vh] w-full max-w-3xl flex-col rounded-md border bg-card shadow-lg">
        <header className="flex items-center justify-between border-b p-3">
          <h3 className="text-sm font-semibold">
            {t("codeWiki.onboard.title", "Onboarding guide")} ·{" "}
            <span className="font-mono text-xs text-muted-foreground">{repoName}</span>
          </h3>
          <div className="flex items-center gap-2">
            {result && (
              <>
                <Button size="sm" variant="ghost" onClick={copyMarkdown}>
                  <Copy className="mr-1 h-3.5 w-3.5" />
                  {copied ? t("codeWiki.onboard.copied", "Copied!") : t("codeWiki.onboard.copy", "Copy")}
                </Button>
                <Button size="sm" variant="ghost" onClick={saveToDocs}>
                  <Save className="mr-1 h-3.5 w-3.5" />
                  {t("codeWiki.onboard.saveToDocs", "Save to docs/")}
                </Button>
              </>
            )}
            <Button variant="ghost" size="icon" onClick={onClose}>
              <X className="h-4 w-4" />
            </Button>
          </div>
        </header>

        <div className="border-b p-3">
          {!result && !loading && !error && (
            <Button onClick={run}>
              <BookOpen className="mr-1 h-3.5 w-3.5" />
              {t("codeWiki.onboard.generate", "Generate onboarding guide")}
            </Button>
          )}
          {loading && (
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <Loader2 className="h-3 w-3 animate-spin" />
              {t("codeWiki.onboard.generating", "Generating…")}
            </div>
          )}
          {error && (
            <div className="text-xs text-muted-foreground">{error}</div>
          )}
          {result && (
            <div className="text-xs text-muted-foreground">
              {result.usedLlm ? "LLM" : "template"} · {result.markdown.length} chars · {result.durationMs}ms
            </div>
          )}
        </div>

        <div className="flex-1 overflow-auto p-4 text-sm">
          {!result && !loading && !error && (
            <p className="text-muted-foreground">
              {t(
                "codeWiki.onboard.empty",
                "Click Generate to produce a 6-section onboarding markdown guide from the knowledge graph.",
              )}
            </p>
          )}
          {result && (
            <div className="prose prose-sm max-w-none dark:prose-invert">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {result.markdown}
              </ReactMarkdown>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}