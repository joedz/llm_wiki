// 3-phase strip for the code-wiki **knowledge** pipeline.
//
// Mirrors `pipeline-progress.tsx` but for the knowledge pipeline:
// 3 phases (Scan + parse / Enrich (LLM) / Save), batch progress
// from Phase 1 (LLM article-analyzer batches), and a final
// KnowledgeStats summary line (articles, claims, entities).

import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { Loader2, CheckCircle2, AlertCircle, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  useKnowledgeStore,
  KNOWLEDGE_PHASE_LABELS,
} from "@/stores/code-wiki-knowledge-store"

interface Props {
  projectPath: string
}

export function KnowledgeProgress({ projectPath }: Props) {
  const { t } = useTranslation()
  const run = useKnowledgeStore((s) => s.byProject[projectPath])
  const reset = useKnowledgeStore((s) => s.reset)
  const [now, setNow] = useState(Date.now())

  useEffect(() => {
    if (!run || run.result !== "running") return
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [run?.result])

  if (!run) return null

  const elapsed = Math.max(0, Math.round((now - run.startedAt) / 1000))
  const isRunning = run.result === "running"
  const batchPct = run.batchTotal > 0
    ? Math.round((run.batchDone / run.batchTotal) * 100)
    : 0

  return (
    <div
      data-testid="knowledge-progress"
      className="rounded-md border bg-card p-3 text-card-foreground shadow-sm"
    >
      <header className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          {isRunning ? (
            <Loader2 className="h-4 w-4 animate-spin text-primary" />
          ) : run.result === "done" ? (
            <CheckCircle2 className="h-4 w-4 text-emerald-600" />
          ) : (
            <AlertCircle className="h-4 w-4 text-destructive" />
          )}
          <span className="text-sm font-semibold">
            {t("codeWiki.knowledge.title", "Knowledge graph")}
            <span className="ml-2 text-xs text-muted-foreground">
              {run.repoName}
            </span>
          </span>
        </div>
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <span>{elapsed}s</span>
          {!isRunning && (
            <Button size="sm" variant="ghost" onClick={() => reset(projectPath)}>
              {t("codeWiki.knowledge.dismiss", "Dismiss")}
            </Button>
          )}
        </div>
      </header>

      <ol className="mt-3 grid grid-cols-1 gap-1 sm:grid-cols-3">
        {KNOWLEDGE_PHASE_LABELS.map((label, idx) => {
          const status =
            idx < run.currentPhase
              ? "done"
              : idx === run.currentPhase
                ? run.phaseStatus
                : "idle"
          return (
            <li
              key={label}
              className={`flex items-center gap-1 rounded px-2 py-1 text-xs ${
                status === "done"
                  ? "bg-emerald-100 text-emerald-800"
                  : status === "running"
                    ? "bg-primary/10 text-primary"
                    : "bg-muted text-muted-foreground"
              }`}
            >
              {status === "done" ? (
                <CheckCircle2 className="h-3 w-3" />
              ) : status === "running" ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <span className="h-3 w-3 rounded-full border" />
              )}
              <span className="truncate">{label}</span>
            </li>
          )
        })}
      </ol>

      {run.batchTotal > 0 && (
        <div className="mt-2 text-xs text-muted-foreground">
          {t("codeWiki.knowledge.batchProgress", {
            defaultValue: "Batch {done}/{total} ({pct}%)",
            done: run.batchDone,
            total: run.batchTotal,
            pct: batchPct,
          })}
        </div>
      )}

      {run.warnings.length > 0 && (
        <details className="mt-2 text-xs text-amber-700">
          <summary className="cursor-pointer">
            {t("codeWiki.knowledge.warnings", {
              defaultValue: "{count} warning(s)",
              count: run.warnings.length,
            })}
          </summary>
          <ul className="mt-1 list-inside list-disc">
            {run.warnings.map((w, i) => (
              <li key={i}>{w}</li>
            ))}
          </ul>
        </details>
      )}

      {run.summary && (
        <p className="mt-2 text-xs text-muted-foreground">
          {t("codeWiki.knowledge.summary", {
            defaultValue:
              "Done in {{durationMs}}ms — {{articles}} articles, {{entities}} entities, {{claims}} claims, {{edges}} edges",
            durationMs: run.summary.durationMs,
            articles: run.summary.stats.articles,
            entities: run.summary.stats.entities,
            claims: run.summary.stats.claims,
            edges: run.summary.stats.edges,
          })}
        </p>
      )}
    </div>
  )
}