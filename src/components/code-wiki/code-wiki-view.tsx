import { useCallback, useEffect, useMemo, useState } from "react"
import { invoke } from "@tauri-apps/api/core"
import { openUrl as openExternal } from "@tauri-apps/plugin-opener"
import {
  Code2,
  RefreshCw,
  PlayCircle,
  ExternalLink,
  Copy,
  CheckCircle2,
  AlertCircle,
  Clock,
  Sparkles,
  GitCompare,
} from "lucide-react"
import { Button } from "@/components/ui/button"
import { useWikiStore } from "@/stores/wiki-store"
import { usePipelineStore } from "@/stores/code-wiki-pipeline-store"
import { startPipeline, llmSpecFromConfig, hasLlmConfig } from "@/lib/code-wiki/pipeline"
import {
  refreshDiffOverlay,
  isOverlayInteresting,
  type DiffOverlay,
} from "@/lib/code-wiki/diff"
import { PipelineProgress } from "./pipeline-progress"
import { normalizePath } from "@/lib/path-utils"
import { useTranslation } from "react-i18next"

interface RepoStatus {
  name: string
  graphPath: string
  lastAnalyzedAt: string
  languages: string[]
  fileCount: number
  symbolCount: number
}

interface OpenDashboardInfo {
  project_path: string
  repo_name: string
  url: string
  port: number
  token: string
}

type BuildState =
  | { kind: "idle" }
  | { kind: "building" }
  | { kind: "error"; message: string }

type OpenState =
  | { kind: "idle" }
  | { kind: "opening" }
  | { kind: "open"; info: OpenDashboardInfo }
  | { kind: "error"; message: string }

function formatRelative(iso: string): string {
  if (!iso) return "—"
  const t = new Date(iso).getTime()
  if (Number.isNaN(t)) return "—"
  const diff = Date.now() - t
  if (diff < 60_000) return "just now"
  if (diff < 3_600_000) return `${Math.round(diff / 60_000)}m ago`
  if (diff < 86_400_000) return `${Math.round(diff / 3_600_000)}h ago`
  return new Date(iso).toLocaleDateString()
}

export function CodeWikiView() {
  const { t } = useTranslation()
  const project = useWikiStore((s) => s.project)
  // Subscribe to pipeline progress events once per app load.
  // (The store deduplicates via pipelineId, so it's safe to call
  // here in addition to anywhere else.)
  useEffect(() => {
    usePipelineStore.getState().startListen()
  }, [])
  const pipelineRun = usePipelineStore((s) =>
    project ? s.byProject[project.path] : undefined,
  )
  const beginPipeline = usePipelineStore((s) => s.begin)
  const [repos, setRepos] = useState<RepoStatus[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [diffByRepo, setDiffByRepo] = useState<Record<string, DiffOverlay | null>>({})
  const [refreshingDiff, setRefreshingDiff] = useState<string | null>(null)
  const [openDiff, setOpenDiff] = useState<Record<string, boolean>>({})
  const [buildStates, setBuildStates] = useState<Record<string, BuildState>>({})
  const [openStates, setOpenStates] = useState<Record<string, OpenState>>({})
  const [copiedRepo, setCopiedRepo] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    if (!project) return
    setLoading(true)
    setError(null)
    try {
      const index = await invoke<{ repos: RepoStatus[] }>("code_wiki_get_index", {
        projectPath: project.path,
      })
      setRepos(index.repos)
    } catch (err) {
      setError(String(err))
      setRepos([])
    } finally {
      setLoading(false)
    }
  }, [project])

  useEffect(() => {
    void refresh()
  }, [refresh])

  // When the project switches, clear cached diffs.
  // `projectPath` is declared further down; lift it here so the
  // effect dependency is defined before use (TDZ triggers TS2448
  // + TS2454 under strict tsc).
  const projectPath = project ? normalizePath(project.path) : ""
  useEffect(() => {
    setDiffByRepo({})
  }, [projectPath])

  const buildRepo = useCallback(
    async (repoName: string) => {
      if (!project) return
      setBuildStates((s) => ({ ...s, [repoName]: { kind: "building" } }))
      try {
        const { buildGraphForRepo } = await import("@/lib/code-wiki")
        await buildGraphForRepo(normalizePath(project.path), repoName)
        setBuildStates((s) => ({ ...s, [repoName]: { kind: "idle" } }))
        await refresh()
      } catch (err) {
        setBuildStates((s) => ({
          ...s,
          [repoName]: { kind: "error", message: String(err) },
        }))
      }
    },
    [project, refresh],
  )

  const openDashboard = useCallback(
    async (repoName: string) => {
      if (!project) return
      setOpenStates((s) => ({ ...s, [repoName]: { kind: "opening" } }))
      try {
        const info = await invoke<OpenDashboardInfo>("code_wiki_open_dashboard", {
          projectPath: project.path,
          repoName,
        })
        setOpenStates((s) => ({ ...s, [repoName]: { kind: "open", info } }))
        await openExternal(info.url)
      } catch (err) {
        setOpenStates((s) => ({
          ...s,
          [repoName]: { kind: "error", message: String(err) },
        }))
      }
    },
    [project],
  )

  const analyzeRepo = useCallback(
    async (repoName: string) => {
      if (!project) return
      const projectPath = normalizePath(project.path)
      beginPipeline(projectPath, repoName)
      const llmConfig = useWikiStore.getState().llmConfig
      const llm = llmSpecFromConfig(llmConfig)
      try {
        await startPipeline(projectPath, repoName, llm ?? undefined)
      } catch (err) {
        // Surface the error via a synthetic warning event so the
        // progress panel reflects the failure rather than hanging
        // forever in "running" state.
        const pipelineId = `pipeline-failed-${Date.now()}`
        usePipelineStore.setState((s) => ({
          byProject: {
            ...s.byProject,
            [projectPath]: {
              ...(s.byProject[projectPath] ?? {
                pipelineId,
                repoName,
                startedAt: Date.now(),
                currentPhase: 0,
                currentPhaseLabel: "Pre-flight",
                phaseStatus: "error" as const,
                batchDone: 0,
                batchTotal: 0,
                warnings: [],
                result: "error" as const,
                summary: null,
                unlisten: null,
              }),
              result: "error" as const,
              phaseStatus: "error" as const,
              warnings: [`startPipeline failed: ${String(err)}`],
            },
          },
        }))
      }
    },
    [project, beginPipeline],
  )

  const refreshDiff = useCallback(
    async (repoName: string) => {
      if (!project) return
      const projectPath = normalizePath(project.path)
      setRefreshingDiff(repoName)
      try {
        const overlay = await refreshDiffOverlay(projectPath, repoName)
        setDiffByRepo((s) => ({ ...s, [repoName]: overlay }))
      } catch (err) {
        setError(`Diff refresh failed for ${repoName}: ${String(err)}`)
      } finally {
        setRefreshingDiff(null)
      }
    },
    [project],
  )

  const copyUrl = useCallback(async (repoName: string) => {
    const state = openStates[repoName]
    if (state?.kind !== "open") return
    try {
      await navigator.clipboard.writeText(state.info.url)
      setCopiedRepo(repoName)
      setTimeout(() => setCopiedRepo(null), 1500)
    } catch {
      /* ignore */
    }
  }, [openStates])

  const summary = useMemo(() => {
    const total = repos.length
    const built = repos.filter((r) => r.lastAnalyzedAt).length
    const totalFiles = repos.reduce((sum, r) => sum + r.fileCount, 0)
    return { total, built, totalFiles }
  }, [repos])

  if (!project) {
    return (
      <div className="flex h-full items-center justify-center text-muted-foreground">
        {t("codeWiki.noProject", "Open a project to see its code graph.")}
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <header className="flex items-center justify-between border-b px-4 py-3">
        <div className="flex items-center gap-2">
          <Code2 className="h-5 w-5" />
          <h2 className="text-lg font-semibold">
            {t("codeWiki.title", "Code Wiki")}
          </h2>
          <span className="text-xs text-muted-foreground">
            · {projectPath}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground">
            {summary.built}/{summary.total} built · {summary.totalFiles} files
          </span>
          <Button
            variant="ghost"
            size="icon"
            onClick={refresh}
            disabled={loading}
            title={t("codeWiki.refresh", "Refresh")}
          >
            <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
          </Button>
        </div>
      </header>

      {error && (
        <div className="border-b bg-destructive/10 px-4 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      {project && pipelineRun && (
        <div className="border-b p-3">
          <PipelineProgress projectPath={project.path} />
        </div>
      )}

      <div className="flex-1 overflow-auto p-4">
        {repos.length === 0 ? (
          <EmptyState onBuild={async () => {
            // Fallback: build any known repos from raw/code/ that aren't
            // in the index yet (i.e. never built).
            try {
              const names = await invoke<string[]>("code_wiki_list_repos", {
                projectPath: project.path,
              })
              for (const name of names) {
                await buildRepo(name)
              }
            } catch (err) {
              setError(String(err))
            }
          }} />
        ) : (
          <ul className="space-y-2">
            {repos.map((repo) => (
              <RepoRow
                key={repo.name}
                repo={repo}
                buildState={buildStates[repo.name] ?? { kind: "idle" }}
                openState={openStates[repo.name] ?? { kind: "idle" }}
                diff={diffByRepo[repo.name] ?? null}
                diffOpen={openDiff[repo.name] ?? false}
                diffRefreshing={refreshingDiff === repo.name}
                onBuild={() => buildRepo(repo.name)}
                onAnalyze={() => analyzeRepo(repo.name)}
                onOpen={() => openDashboard(repo.name)}
                onCopyUrl={() => copyUrl(repo.name)}
                onToggleDiff={() =>
                  setOpenDiff((s) => ({ ...s, [repo.name]: !(s[repo.name] ?? false) }))
                }
                onRefreshDiff={() => refreshDiff(repo.name)}
                copied={copiedRepo === repo.name}
              />
            ))}
          </ul>
        )}
      </div>
    </div>
  )
}

function EmptyState({ onBuild }: { onBuild: () => Promise<void> }) {
  const { t } = useTranslation()
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 text-center text-muted-foreground">
      <Code2 className="h-12 w-12" />
      <p className="text-sm">
        {t("codeWiki.empty", "No code repos indexed yet. Import code in Sources, then build a graph.")}
      </p>
      <Button onClick={onBuild}>{t("codeWiki.detectAndBuild", "Detect & build")}</Button>
    </div>
  )
}

function RepoRow({
  repo,
  buildState,
  openState,
  diff,
  diffOpen,
  diffRefreshing,
  onBuild,
  onAnalyze,
  onOpen,
  onCopyUrl,
  onToggleDiff,
  onRefreshDiff,
  copied,
}: {
  repo: RepoStatus
  buildState: BuildState
  openState: OpenState
  diff: DiffOverlay | null
  diffOpen: boolean
  diffRefreshing: boolean
  onAnalyze: () => void
  onBuild: () => void
  onOpen: () => void
  onCopyUrl: () => void
  onToggleDiff: () => void
  onRefreshDiff: () => void
  copied: boolean
}) {
  const { t } = useTranslation()
  const built = Boolean(repo.lastAnalyzedAt)
  const langs = repo.languages.length > 0 ? repo.languages.join(", ") : "—"
  const analyzing = usePipelineStore((s) =>
    Object.values(s.byProject).some(
      (run) => run.repoName === repo.name && run.result === "running",
    ),
  )
  return (
    <li className="rounded-md border bg-card p-3 text-card-foreground shadow-sm">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="font-mono text-sm font-semibold">{repo.name}</span>
            <span className="text-xs text-muted-foreground">
              <Clock className="mr-1 inline h-3 w-3" />
              {formatRelative(repo.lastAnalyzedAt)}
            </span>
          </div>
          <div className="mt-1 text-xs text-muted-foreground">
            {langs} · {repo.fileCount} files · {repo.symbolCount} symbols
          </div>
          {buildState.kind === "error" && (
            <div className="mt-1 flex items-center gap-1 text-xs text-destructive">
              <AlertCircle className="h-3 w-3" />
              {buildState.message}
            </div>
          )}
          {openState.kind === "error" && (
            <div className="mt-1 flex items-center gap-1 text-xs text-destructive">
              <AlertCircle className="h-3 w-3" />
              {openState.message}
            </div>
          )}
          {openState.kind === "open" && (
            <div className="mt-1 flex items-center gap-1 text-xs text-emerald-600">
              <CheckCircle2 className="h-3 w-3" />
              {t("codeWiki.running", "Dashboard running")} · :{openState.info.port}
            </div>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={onBuild}
            disabled={buildState.kind === "building"}
            title={t("codeWiki.buildTooltip", "Re-run codegraph index")}
          >
            <PlayCircle className="mr-1 h-3.5 w-3.5" />
            {buildState.kind === "building"
              ? t("codeWiki.building", "Building…")
              : t("codeWiki.build", built ? "Rebuild" : "Build")}
          </Button>
          <Button
            variant="secondary"
            size="sm"
            onClick={onAnalyze}
            disabled={analyzing}
            title={t("codeWiki.analyzeTooltip", "Run the 7-phase analysis pipeline (preflight, scan, batch, save)")}
          >
            <Sparkles className="mr-1 h-3.5 w-3.5" />
            {analyzing
              ? t("codeWiki.analyzing", "Analyzing…")
              : hasLlmConfig(useWikiStore.getState().llmConfig)
                ? t("codeWiki.analyzeWithLlm", "Analyze (LLM)")
                : t("codeWiki.analyze", "Analyze")}
          </Button>
          <Button
            variant="default"
            size="sm"
            onClick={onOpen}
            disabled={openState.kind === "opening"}
            title={t("codeWiki.openTooltip", "Open dashboard in browser")}
          >
            <ExternalLink className="mr-1 h-3.5 w-3.5" />
            {t("codeWiki.open", "Open Dashboard")}
          </Button>
          {openState.kind === "open" && (
            <Button
              variant="ghost"
              size="icon"
              onClick={onCopyUrl}
              title={t("codeWiki.copyUrl", "Copy URL")}
            >
              {copied ? (
                <CheckCircle2 className="h-3.5 w-3.5 text-emerald-600" />
              ) : (
                <Copy className="h-3.5 w-3.5" />
              )}
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            onClick={onToggleDiff}
            disabled={!built}
            title={t("codeWiki.diffTooltip", "Show working-tree changes vs the graph")}
          >
            <GitCompare className="mr-1 h-3.5 w-3.5" />
            {diffOpen ? t("codeWiki.hideDiff", "Hide diff") : t("codeWiki.showDiff", "Diff")}
            {diff && isOverlayInteresting(diff) && (
              <span className="ml-1 rounded-full bg-amber-500 px-1.5 text-[10px] font-semibold text-white">
                {diff.changedNodeIds.length + diff.affectedNodeIds.length}
              </span>
            )}
          </Button>
        </div>
      </div>
      {diffOpen && (
        <DiffPanel
          overlay={diff}
          refreshing={diffRefreshing}
          onRefresh={onRefreshDiff}
        />
      )}
    </li>
  )
}

function DiffPanel({
  overlay,
  refreshing,
  onRefresh,
}: {
  overlay: DiffOverlay | null
  refreshing: boolean
  onRefresh: () => void
}) {
  const { t } = useTranslation()
  if (!overlay) {
    return (
      <div className="mt-2 rounded border bg-muted/40 p-3 text-xs text-muted-foreground">
        {refreshing
          ? t("codeWiki.diffLoading", "Loading diff overlay…")
          : t("codeWiki.diffEmpty", "No overlay yet. Click refresh to compute.")}
      </div>
    )
  }
  return (
    <div className="mt-2 rounded border bg-muted/40 p-3 text-xs">
      <div className="mb-2 flex items-center justify-between">
        <span className="text-muted-foreground">
          {t("codeWiki.diffBase", "Base")}: {overlay.baseBranch} ·{" "}
          {t("codeWiki.diffGenerated", "generated")}{" "}
          {formatRelative(overlay.generatedAt)}
        </span>
        <Button
          variant="ghost"
          size="sm"
          onClick={onRefresh}
          disabled={refreshing}
        >
          <RefreshCw className={`mr-1 h-3 w-3 ${refreshing ? "animate-spin" : ""}`} />
          {t("codeWiki.diffRefresh", "Refresh")}
        </Button>
      </div>
      {!isOverlayInteresting(overlay) ? (
        <div className="text-muted-foreground">
          {t("codeWiki.diffNothing", "No working-tree changes detected.")}
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-3">
          <DiffColumn
            label={t("codeWiki.diffChanged", "Changed")}
            ids={overlay.changedNodeIds}
            color="amber"
          />
          <DiffColumn
            label={t("codeWiki.diffAffected", "Affected (1-hop)")}
            ids={overlay.affectedNodeIds}
            color="sky"
          />
        </div>
      )}
      {overlay.warnings.length > 0 && (
        <div className="mt-2 text-muted-foreground">
          {overlay.warnings.map((w, i) => (
            <div key={i}>⚠ {w}</div>
          ))}
        </div>
      )}
    </div>
  )
}

function DiffColumn({
  label,
  ids,
  color,
}: {
  label: string
  ids: string[]
  color: "amber" | "sky"
}) {
  const { t } = useTranslation()
  const dotColor = color === "amber" ? "bg-amber-500" : "bg-sky-500"
  if (ids.length === 0) {
    return (
      <div>
        <div className="mb-1 font-semibold">{label}</div>
        <div className="text-muted-foreground">
          {t("codeWiki.diffNone", "none")}
        </div>
      </div>
    )
  }
  return (
    <div>
      <div className="mb-1 font-semibold">
        {label} <span className="text-muted-foreground">({ids.length})</span>
      </div>
      <ul className="space-y-1">
        {ids.slice(0, 12).map((id) => (
          <li key={id} className="flex items-center gap-2 font-mono">
            <span className={`inline-block h-1.5 w-1.5 rounded-full ${dotColor}`} />
            <span className="truncate">{id}</span>
          </li>
        ))}
        {ids.length > 12 && (
          <li className="text-muted-foreground">
            {t("codeWiki.diffMore", "+{n} more", { n: ids.length - 12 })}
          </li>
        )}
      </ul>
    </div>
  )
}
