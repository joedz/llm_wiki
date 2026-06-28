import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { Loader2, CheckCircle2, AlertCircle, X, StopCircle } from "lucide-react"
import { Button } from "@/components/ui/button"
import { usePipelineStore, PIPELINE_PHASE_LABELS } from "@/stores/code-wiki-pipeline-store"
import { cancelPipeline } from "@/lib/code-wiki/pipeline"

interface Props {
  projectPath: string
}

export function PipelineProgress({ projectPath }: Props) {
  const { t } = useTranslation()
  const run = usePipelineStore((s) => s.byProject[projectPath])
  const reset = usePipelineStore((s) => s.reset)
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
      data-testid="pipeline-progress"
      className="rounded-md border bg-card p-3 text-card-foreground shadow-sm"
    >
      <header className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          {isRunning ? (
            <Loader2 className="h-4 w-4 animate-spin text-primary" />
          ) : run.result === "done" ? (
            <CheckCircle2 className="h-4 w-4 text-emerald-600" />
          ) : run.result === "cancelled" ? (
            <StopCircle className="h-4 w-4 text-muted-foreground" />
          ) : (
            <AlertCircle className="h-4 w-4 text-destructive" />
          )}
          <span className="text-sm font-semibold">
            {t("codeWiki.pipeline.title", "Pipeline")}
            <span className="ml-2 text-xs text-muted-foreground">
              {run.repoName}
            </span>
          </span>
        </div>
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <span>{elapsed}s</span>
          {isRunning && (
            <Button
              size="sm"
              variant="outline"
              onClick={() => cancelPipeline(run.pipelineId)}
            >
              <X className="mr-1 h-3.5 w-3.5" />
              {t("codeWiki.pipeline.cancel", "Cancel")}
            </Button>
          )}
          {!isRunning && (
            <Button size="sm" variant="ghost" onClick={() => reset(projectPath)}>
              {t("codeWiki.pipeline.dismiss", "Dismiss")}
            </Button>
          )}
        </div>
      </header>

      <ol className="mt-3 grid grid-cols-1 gap-1 sm:grid-cols-4 lg:grid-cols-8">
        {PIPELINE_PHASE_LABELS.map((label, idx) => {
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
          {t("codeWiki.pipeline.batchProgress", {
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
            {t("codeWiki.pipeline.warnings", {
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
          {t("codeWiki.pipeline.summary", {
            defaultValue:
              "Done in {durationMs}ms — {nodeCount} nodes, {edgeCount} edges",
            durationMs: run.summary.durationMs,
            nodeCount: run.summary.nodeCount,
            edgeCount: run.summary.edgeCount,
          })}
          {run.summary.cancelled ? " (cancelled)" : ""}
        </p>
      )}
    </div>
  )
}
