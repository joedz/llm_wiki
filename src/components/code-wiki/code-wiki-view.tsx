import { useCallback, useEffect, useMemo, useState } from "react"
import { invoke } from "@tauri-apps/api/core"
import { open as openExternal } from "@tauri-apps/plugin-opener"
import {
  Code2,
  RefreshCw,
  PlayCircle,
  ExternalLink,
  Copy,
  CheckCircle2,
  AlertCircle,
  Clock,
} from "lucide-react"
import { Button } from "@/components/ui/button"
import { useWikiStore } from "@/stores/wiki-store"
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
  const [repos, setRepos] = useState<RepoStatus[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
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

  const projectPath = project ? normalizePath(project.path) : ""
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
                onBuild={() => buildRepo(repo.name)}
                onOpen={() => openDashboard(repo.name)}
                onCopyUrl={() => copyUrl(repo.name)}
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
  onBuild,
  onOpen,
  onCopyUrl,
  copied,
}: {
  repo: RepoStatus
  buildState: BuildState
  openState: OpenState
  onBuild: () => void
  onOpen: () => void
  onCopyUrl: () => void
  copied: boolean
}) {
  const { t } = useTranslation()
  const built = Boolean(repo.lastAnalyzedAt)
  const langs = repo.languages.length > 0 ? repo.languages.join(", ") : "—"
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
        </div>
      </div>
    </li>
  )
}
