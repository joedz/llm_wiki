// Explain modal — pick a node by id, get a deep-dive explanation.
//
// Two-step flow:
//   1. User enters a node id (or picks from autocomplete of file nodes)
//   2. We invoke `code_wiki_explain_node` and render the returned
//      markdown via `react-markdown`.
//
// The autocomplete pulls up to 50 file nodes from the knowledge
// graph so the user does not have to type a raw id like
// `function:src/auth.ts:verifyToken`.

import { useEffect, useMemo, useState } from "react"
import { invoke } from "@tauri-apps/api/core"
import ReactMarkdown from "react-markdown"
import remarkGfm from "remark-gfm"
import { useTranslation } from "react-i18next"
import { Loader2, Sparkles, X } from "lucide-react"
import { Button } from "@/components/ui/button"
import { explainNode, type ExplainResult } from "@/lib/code-wiki/explain"
import { llmSpecFromConfig } from "@/lib/code-wiki/pipeline"
import { useWikiStore } from "@/stores/wiki-store"
import { normalizePath } from "@/lib/path-utils"

interface Props {
  open: boolean
  projectPath: string
  repoName: string
  onClose: () => void
}

export function ExplainModal({ open, projectPath, repoName, onClose }: Props) {
  const { t } = useTranslation()
  const [nodeId, setNodeId] = useState("")
  const [nodeOptions, setNodeOptions] = useState<string[]>([])
  const [loading, setLoading] = useState(false)
  const [result, setResult] = useState<ExplainResult | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!open) return
    // Fetch the knowledge graph and pull up to 50 file node ids
    // for autocomplete.
    void (async () => {
      try {
        const graph = (await invoke("code_wiki_get_graph", {
          projectPath: normalizePath(projectPath),
          repoName,
        })) as { nodes?: { id: string; type: string }[] } | null
        if (!graph || !graph.nodes) return
        const ids = graph.nodes
          .filter((n) => n.type === "file" || n.type === "function" || n.type === "class")
          .slice(0, 50)
          .map((n) => n.id)
        setNodeOptions(ids)
      } catch {
        // Graph may not exist yet — leave empty
      }
    })()
  }, [open, projectPath, repoName])

  const submit = async () => {
    const id = nodeId.trim()
    if (!id) return
    setLoading(true)
    setError(null)
    setResult(null)
    try {
      const llmConfig = useWikiStore.getState().llmConfig
      const llm = llmSpecFromConfig(llmConfig)
      const r = await explainNode(normalizePath(projectPath), repoName, id, llm ?? undefined)
      setResult(r)
    } catch (err) {
      setError(String(err))
    } finally {
      setLoading(false)
    }
  }

  const filteredOptions = useMemo(() => {
    if (!nodeId) return nodeOptions.slice(0, 10)
    const lower = nodeId.toLowerCase()
    return nodeOptions.filter((o) => o.toLowerCase().includes(lower)).slice(0, 10)
  }, [nodeId, nodeOptions])

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-4"
      role="dialog"
      aria-modal="true"
      data-testid="explain-modal"
    >
      <div className="flex max-h-[85vh] w-full max-w-3xl flex-col rounded-md border bg-card shadow-lg">
        <header className="flex items-center justify-between border-b p-3">
          <h3 className="text-sm font-semibold">
            {t("codeWiki.explain.title", "Explain a node")} ·{" "}
            <span className="font-mono text-xs text-muted-foreground">{repoName}</span>
          </h3>
          <Button variant="ghost" size="icon" onClick={onClose}>
            <X className="h-4 w-4" />
          </Button>
        </header>

        <div className="space-y-2 border-b p-3">
          <label className="block text-xs font-medium text-muted-foreground">
            {t("codeWiki.explain.nodeIdLabel", "Node ID (file / function / class / ...)")}
          </label>
          <input
            value={nodeId}
            onChange={(e) => setNodeId(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !loading) void submit()
            }}
            placeholder="function:src/auth.ts:verifyToken"
            className="w-full rounded border bg-background px-2 py-1 text-sm font-mono"
            disabled={loading}
          />
          {filteredOptions.length > 0 && (
            <div className="flex flex-wrap gap-1 text-xs">
              {filteredOptions.map((id) => (
                <button
                  key={id}
                  type="button"
                  onClick={() => setNodeId(id)}
                  className="rounded bg-muted px-2 py-0.5 font-mono hover:bg-accent"
                >
                  {id}
                </button>
              ))}
            </div>
          )}
          <div className="flex items-center gap-2">
            <Button onClick={submit} disabled={loading || !nodeId.trim()} size="sm">
              {loading ? (
                <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
              ) : (
                <Sparkles className="mr-1 h-3.5 w-3.5" />
              )}
              {t("codeWiki.explain.run", "Explain")}
            </Button>
            {result && (
              <span className="text-xs text-muted-foreground">
                {result.usedLlm ? "LLM" : "template"} · {result.neighborCount} neighbors ·{" "}
                {result.sourceLinesRead} source lines · {result.durationMs}ms
              </span>
            )}
          </div>
        </div>

        <div className="flex-1 overflow-auto p-4 text-sm">
          {error && (
            <div className="rounded border border-destructive/50 bg-destructive/10 p-2 text-destructive">
              {error}
            </div>
          )}
          {!error && !result && !loading && (
            <p className="text-muted-foreground">
              {t(
                "codeWiki.explain.empty",
                "Enter a node id above (or pick from suggestions) and press Explain.",
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