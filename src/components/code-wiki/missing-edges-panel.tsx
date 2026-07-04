// P3-A + P4-A: Missing-edges panel. Shows actionable suggestions
// from the graph reviewer in a modal. P4-A adds the "Auto-fix"
// button that calls the LLM-driven auto-fix Tauri command.

import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { AlertTriangle, AlertCircle, Info, Loader2, X, Lightbulb, Wand2, Check } from "lucide-react"
import {
  autoFixMissingEdges,
  getMissingEdges,
  type AutoFixReport,
  type MissingEdgeSuggestion,
} from "@/lib/code-wiki/review"
import { useWikiStore } from "@/stores/wiki-store"
import { llmSpecFromConfig } from "@/lib/code-wiki/pipeline"
import { normalizePath } from "@/lib/path-utils"
import { Button } from "@/components/ui/button"

interface Props {
  open: boolean
  projectPath: string
  repoName: string
  onClose: () => void
}

const SEVERITY_ICON: Record<
  MissingEdgeSuggestion["severity"],
  React.ComponentType<{ className?: string }>
> = {
  error: AlertCircle,
  warning: AlertTriangle,
  info: Info,
}

const SEVERITY_COLOR: Record<MissingEdgeSuggestion["severity"], string> = {
  error: "text-red-500",
  warning: "text-amber-500",
  info: "text-blue-500",
}

export function MissingEdgesPanel({ open, projectPath, repoName, onClose }: Props) {
  const { t } = useTranslation()
  const llmConfig = useWikiStore((s) => s.llmConfig)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [suggestions, setSuggestions] = useState<MissingEdgeSuggestion[] | null>(null)
  const [fixRunning, setFixRunning] = useState(false)
  const [fixReport, setFixReport] = useState<AutoFixReport | null>(null)
  const [fixError, setFixError] = useState<string | null>(null)

  useEffect(() => {
    if (!open) {
      setSuggestions(null)
      setError(null)
      setFixReport(null)
      setFixError(null)
      return
    }
    setLoading(true)
    setError(null)
    ;(async () => {
      try {
        const result = await getMissingEdges(normalizePath(projectPath), repoName)
        setSuggestions(result)
      } catch (e) {
        setError(String(e))
      } finally {
        setLoading(false)
      }
    })()
  }, [open, projectPath, repoName])

  const runAutoFix = async (ruleIds: string[] | null) => {
    setFixRunning(true)
    setFixError(null)
    setFixReport(null)
    try {
      const llm = llmSpecFromConfig(llmConfig) ?? null
      const report = await autoFixMissingEdges(
        normalizePath(projectPath),
        repoName,
        ruleIds,
        llm,
      )
      setFixReport(report)
      // Re-fetch the (now-updated) missing-edges list
      const result = await getMissingEdges(normalizePath(projectPath), repoName)
      setSuggestions(result)
    } catch (e) {
      setFixError(String(e))
    } finally {
      setFixRunning(false)
    }
  }

  if (!open) return null

  const grouped = groupBySeverity(suggestions ?? [])

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-4"
      role="dialog"
      aria-modal="true"
      data-testid="missing-edges-panel"
    >
      <div className="flex max-h-[85vh] w-full max-w-3xl flex-col rounded-md border bg-card shadow-lg">
        <header className="flex items-center justify-between border-b p-3">
          <h3 className="flex items-center gap-2 text-sm font-semibold">
            <Lightbulb className="h-4 w-4" />
            {t("codeWiki.missingEdges.title", "Missing Edges")} ·{" "}
            <span className="font-mono text-xs text-muted-foreground">{repoName}</span>
          </h3>
          <div className="flex items-center gap-1">
            {suggestions && suggestions.length > 0 && (
              <Button
                size="sm"
                variant="default"
                onClick={() => runAutoFix(null)}
                disabled={fixRunning}
                data-testid="auto-fix-all-button"
              >
                {fixRunning ? (
                  <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Wand2 className="mr-1 h-3.5 w-3.5" />
                )}
                {t("codeWiki.missingEdges.autoFixAll", "Auto-fix all")}
              </Button>
            )}
            <Button variant="ghost" size="icon" onClick={onClose}>
              <X className="h-4 w-4" />
            </Button>
          </div>
        </header>

        <div className="flex-1 overflow-auto p-4">
          {loading && (
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <Loader2 className="h-3 w-3 animate-spin" />
              {t("codeWiki.missingEdges.loading", "Loading suggestions…")}
            </div>
          )}
          {error && <div className="text-xs text-red-500">{error}</div>}
          {!loading && !error && suggestions && suggestions.length === 0 && (
            <div className="text-xs text-muted-foreground">
              {t(
                "codeWiki.missingEdges.empty",
                "No missing edges detected. Graph looks healthy.",
              )}
            </div>
          )}
          {!loading && !error && suggestions && suggestions.length > 0 && (
            <div className="space-y-4">
              {(["error", "warning", "info"] as const).map((sev) => {
                const items = grouped[sev] ?? []
                if (items.length === 0) return null
                return (
                  <div key={sev} data-testid={`missing-edges-group-${sev}`}>
                    <div className="mb-1 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      {sev} · {items.length}
                    </div>
                    <ul className="space-y-1.5">
                      {items.map((s, i) => (
                        <SuggestionRow key={`${s.ruleId}-${i}`} s={s} />
                      ))}
                    </ul>
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {suggestions && suggestions.length > 0 && (
          <footer className="flex items-center justify-between border-t p-2 text-xs text-muted-foreground">
            <span>
              {t("codeWiki.missingEdges.totalCount", "Total: {{count}}", {
                count: suggestions.length,
              })}
            </span>
            <span>
              {t(
                "codeWiki.missingEdges.fixHint",
                "Click Auto-fix to add edges via LLM",
              )}
            </span>
          </footer>
        )}

        {/* P4-A: auto-fix result banner */}
        {(fixReport || fixError) && (
          <div
            className="border-t bg-muted/30 p-3 text-xs"
            data-testid="auto-fix-result"
          >
            {fixError && (
              <div className="text-red-500">
                {t("codeWiki.missingEdges.autoFixError", "Auto-fix failed")}:{" "}
                {fixError}
              </div>
            )}
            {fixReport && (
              <div className="flex items-center gap-2 text-foreground">
                <Check className="h-3.5 w-3.5 text-green-500" />
                <span>
                  {t(
                    "codeWiki.missingEdges.autoFixSuccess",
                    "Added {{added}} edges, dismissed {{dismissed}}, {{remaining}} remaining",
                    {
                      added: fixReport.edgesAdded,
                      dismissed: fixReport.dismissed,
                      remaining: fixReport.remaining,
                    },
                  )}
                </span>
              </div>
            )}
            {fixReport && fixReport.newEdges.length > 0 && (
              <ul className="mt-1 space-y-0.5 font-mono text-[10px] text-muted-foreground">
                {fixReport.newEdges.map((e, i) => (
                  <li key={i}>
                    {e.source} --{e.kind}→ {e.target}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function SuggestionRow({ s }: { s: MissingEdgeSuggestion }) {
  const Icon = SEVERITY_ICON[s.severity]
  return (
    <li
      className="flex items-start gap-2 rounded-md border bg-background/50 p-2 text-xs"
      data-testid={`missing-edge-row-${s.ruleId}`}
    >
      <Icon className={`mt-0.5 h-3.5 w-3.5 shrink-0 ${SEVERITY_COLOR[s.severity]}`} />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-1.5">
          <span className="font-mono text-[10px] text-muted-foreground">
            {s.ruleId}
          </span>
          <span className="text-muted-foreground">·</span>
          <span className="font-medium">{s.edgeKind}</span>
          {s.filePath && (
            <>
              <span className="text-muted-foreground">·</span>
              <span className="truncate font-mono text-[10px] text-muted-foreground">
                {s.filePath}
              </span>
            </>
          )}
        </div>
        <div className="mt-0.5 text-foreground">{s.description}</div>
      </div>
    </li>
  );
}

function groupBySeverity(
  list: MissingEdgeSuggestion[],
): Record<MissingEdgeSuggestion["severity"], MissingEdgeSuggestion[]> {
  const out: Record<
    MissingEdgeSuggestion["severity"],
    MissingEdgeSuggestion[]
  > = { error: [], warning: [], info: [] };
  for (const s of list) {
    out[s.severity].push(s);
  }
  return out;
}