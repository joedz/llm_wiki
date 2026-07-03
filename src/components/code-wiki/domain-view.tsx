// P1-C: Lightweight SVG domain view.
//
// Renders the `domain-graph.json` produced by the domain pipeline.
// Two modes:
//   - Overview (default): all `domain` nodes + cross-domain edges.
//   - Detail (after click): the chosen domain's flows + their steps.
//
// Layout is intentionally simple — no `@xyflow/react` / ELK dependency
// — because domain graphs are small (5-20 nodes typically). Flows
// are placed in a horizontal row; their steps are placed below in a
// second row, ordered by the `weight` field on `flow_step` edges.

import { useEffect, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { Loader2, Network, X, ArrowLeft } from "lucide-react"
import {
  getDomainGraph,
  type DomainGraph,
  type DomainGraphEdge,
  type DomainGraphNode,
} from "@/lib/code-wiki/domain"
import { normalizePath } from "@/lib/path-utils"
import { Button } from "@/components/ui/button"

interface Props {
  open: boolean
  projectPath: string
  repoName: string
  onClose: () => void
}

const NODE_W = 220
const NODE_H = 64
const STEP_W = 180
const STEP_H = 52
const GAP_X = 32
const GAP_Y = 56

export function DomainView({ open, projectPath, repoName, onClose }: Props) {
  const { t } = useTranslation()
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [graph, setGraph] = useState<DomainGraph | null>(null)
  const [activeDomainId, setActiveDomainId] = useState<string | null>(null)

  useEffect(() => {
    if (!open) {
      setGraph(null)
      setActiveDomainId(null)
      setError(null)
      return
    }
    setLoading(true)
    setError(null)
    ;(async () => {
      try {
        const g = await getDomainGraph(normalizePath(projectPath), repoName)
        if (!g) {
          setError("No domain graph found. Run /understand-domain first.")
          setGraph(null)
        } else {
          setGraph(g as DomainGraph)
        }
      } catch (e) {
        setError(String(e))
      } finally {
        setLoading(false)
      }
    })()
  }, [open, projectPath, repoName])

  const view = useMemo(() => {
    if (!graph) return null
    if (activeDomainId) {
      return buildDomainDetail(graph, activeDomainId)
    }
    return buildDomainOverview(graph)
  }, [graph, activeDomainId])

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-4"
      role="dialog"
      aria-modal="true"
      data-testid="domain-view"
    >
      <div className="flex max-h-[90vh] w-full max-w-5xl flex-col rounded-md border bg-card shadow-lg">
        <header className="flex items-center justify-between border-b p-3">
          <h3 className="flex items-center gap-2 text-sm font-semibold">
            <Network className="h-4 w-4" />
            {t("codeWiki.domainView.title", "Domain View")} ·{" "}
            <span className="font-mono text-xs text-muted-foreground">
              {repoName}
            </span>
          </h3>
          <div className="flex items-center gap-2">
            {activeDomainId && (
              <Button
                size="sm"
                variant="ghost"
                onClick={() => setActiveDomainId(null)}
              >
                <ArrowLeft className="mr-1 h-3.5 w-3.5" />
                {t("codeWiki.domainView.back", "Back to domains")}
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
              {t("codeWiki.domainView.loading", "Loading domain graph…")}
            </div>
          )}
          {error && <div className="text-xs text-red-500">{error}</div>}
          {view && <DomainSvg view={view} />}
          {!view && !loading && !error && (
            <p className="text-xs text-muted-foreground">
              {t(
                "codeWiki.domainView.empty",
                "No domain graph available. Run the domain pipeline first.",
              )}
            </p>
          )}
        </div>

        {graph && (
          <footer className="flex items-center justify-between border-t p-2 text-xs text-muted-foreground">
            <span>
              {graph.nodes.length} nodes · {graph.edges.length} edges
            </span>
            <span>
              {t("codeWiki.domainView.dashboardHint", "Open dashboard for full view")}
            </span>
          </footer>
        )}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

interface BuiltView {
  width: number
  height: number
  domainRects: Array<{
    node: DomainGraphNode
    x: number
    y: number
    flowCount: number
  }>
  crossDomainEdges: Array<{
    edge: DomainGraphEdge
    fromX: number
    fromY: number
    toX: number
    toY: number
  }>
  flowRects: Array<{
    node: DomainGraphNode
    x: number
    y: number
    stepCount: number
  }>
  stepRects: Array<{
    node: DomainGraphNode
    x: number
    y: number
    order: number
  }>
  flowStepEdges: Array<{
    edge: DomainGraphEdge
    fromX: number
    fromY: number
    toX: number
    toY: number
    weight: number
  }>
  mode: "overview" | "detail"
}

function buildDomainOverview(graph: DomainGraph): BuiltView {
  const domainNodes = graph.nodes.filter((n) => n.type === "domain")
  const flowCountMap = new Map<string, number>()
  for (const e of graph.edges) {
    if (e.type === "contains_flow") {
      flowCountMap.set(e.source, (flowCountMap.get(e.source) ?? 0) + 1)
    }
  }

  const padding = 24
  const cols = Math.max(1, Math.ceil(Math.sqrt(domainNodes.length)))
  const rows = Math.max(1, Math.ceil(domainNodes.length / cols))
  const width = padding * 2 + cols * (NODE_W + GAP_X)
  const height = padding * 2 + rows * (NODE_H + GAP_Y)

  const domainRects = domainNodes.map((node, i) => {
    const col = i % cols
    const row = Math.floor(i / cols)
    return {
      node,
      x: padding + col * (NODE_W + GAP_X),
      y: padding + row * (NODE_H + GAP_Y),
      flowCount: flowCountMap.get(node.id) ?? 0,
    }
  })

  const rectIndex = new Map<string, { x: number; y: number }>()
  for (const r of domainRects) rectIndex.set(r.node.id, r)

  const crossDomainEdges: BuiltView["crossDomainEdges"] = []
  for (const e of graph.edges) {
    if (e.type !== "cross_domain") continue
    const from = rectIndex.get(e.source)
    const to = rectIndex.get(e.target)
    if (!from || !to) continue
    crossDomainEdges.push({
      edge: e,
      fromX: from.x + NODE_W / 2,
      fromY: from.y + NODE_H / 2,
      toX: to.x + NODE_W / 2,
      toY: to.y + NODE_H / 2,
    })
  }

  return {
    width,
    height,
    domainRects,
    crossDomainEdges,
    flowRects: [],
    stepRects: [],
    flowStepEdges: [],
    mode: "overview",
  }
}

function buildDomainDetail(graph: DomainGraph, domainId: string): BuiltView {
  // flows under this domain
  const flowIds = new Set(
    graph.edges
      .filter((e) => e.type === "contains_flow" && e.source === domainId)
      .map((e) => e.target),
  )
  const flowNodes = graph.nodes.filter((n) => flowIds.has(n.id))

  // step edges from those flows
  const stepEdges = graph.edges.filter(
    (e) => e.type === "flow_step" && flowIds.has(e.source),
  )
  const stepIds = new Set(stepEdges.map((e) => e.target))
  const stepNodes = graph.nodes.filter((n) => stepIds.has(n.id))

  // ordering: step order = round(weight * 10)
  const stepOrderMap = new Map<string, number>()
  for (const e of stepEdges) {
    stepOrderMap.set(e.target, Math.round((e.weight ?? 0) * 10))
  }

  // group steps by flow
  const stepsByFlow = new Map<string, DomainGraphNode[]>()
  for (const e of stepEdges) {
    const arr = stepsByFlow.get(e.source) ?? []
    const node = stepNodes.find((n) => n.id === e.target)
    if (node) arr.push(node)
    stepsByFlow.set(e.source, arr)
  }
  // sort each group's steps by weight asc
  for (const [flowId, steps] of stepsByFlow) {
    steps.sort((a, b) => {
      const wa = stepEdges.find((e) => e.target === a.id)?.weight ?? 0
      const wb = stepEdges.find((e) => e.target === b.id)?.weight ?? 0
      return wa - wb
    })
    stepsByFlow.set(flowId, steps)
  }

  const padding = 24
  // Flow row at y=padding; step row at y=padding + NODE_H + GAP_Y
  const flowY = padding
  const stepY = padding + NODE_H + GAP_Y * 2

  const flowRects: BuiltView["flowRects"] = flowNodes.map((node, i) => ({
    node,
    x: padding + i * (NODE_W + GAP_X),
    y: flowY,
    stepCount: stepsByFlow.get(node.id)?.length ?? 0,
  }))
  const flowRectMap = new Map<string, { x: number; y: number }>()
  for (const r of flowRects) flowRectMap.set(r.node.id, r)

  const stepRects: BuiltView["stepRects"] = []
  // Steps get their own row, ordered by their flow's column then by order
  let stepCursor = padding
  for (const flow of flowNodes) {
    const steps = stepsByFlow.get(flow.id) ?? []
    for (const s of steps) {
      stepRects.push({
        node: s,
        x: stepCursor,
        y: stepY,
        order: stepOrderMap.get(s.id) ?? 0,
      })
      stepCursor += STEP_W + GAP_X
    }
  }
  const stepRectMap = new Map<string, { x: number; y: number }>()
  for (const r of stepRects) stepRectMap.set(r.node, r)

  const flowStepEdges: BuiltView["flowStepEdges"] = []
  for (const e of stepEdges) {
    const from = flowRectMap.get(e.source)
    const to = stepRectMap.get(e.target)
    if (!from || !to) continue
    flowStepEdges.push({
      edge: e,
      fromX: from.x + NODE_W / 2,
      fromY: from.y + NODE_H,
      toX: to.x + STEP_W / 2,
      toY: to.y,
      weight: e.weight,
    })
  }

  const width = Math.max(
    padding * 2 + flowNodes.length * (NODE_W + GAP_X),
    padding * 2 + stepCursor - GAP_X,
  )
  const height = padding * 2 + NODE_H + GAP_Y * 2 + STEP_H + GAP_Y

  return {
    width,
    height,
    domainRects: [],
    crossDomainEdges: [],
    flowRects,
    stepRects,
    flowStepEdges,
    mode: "detail",
  }
}

// ---------------------------------------------------------------------------
// SVG renderer
// ---------------------------------------------------------------------------

function DomainSvg({ view }: { view: BuiltView }) {
  if (view.mode === "overview") {
    return (
      <svg
        width={view.width}
        height={view.height}
        viewBox={`0 0 ${view.width} ${view.height}`}
        className="text-card-foreground"
        data-testid="domain-overview-svg"
      >
        <defs>
          <marker
            id="cd-arrow"
            viewBox="0 0 10 10"
            refX="9"
            refY="5"
            markerWidth="6"
            markerHeight="6"
            orient="auto"
          >
            <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--color-accent)" />
          </marker>
        </defs>
        {view.crossDomainEdges.map((e, i) => (
          <line
            key={`cd-${i}`}
            x1={e.fromX}
            y1={e.fromY}
            x2={e.toX}
            y2={e.toY}
            stroke="var(--color-accent)"
            strokeWidth={2}
            strokeDasharray="6 3"
            markerEnd="url(#cd-arrow)"
          >
            <title>{e.edge.description ?? "cross-domain"}</title>
          </line>
        ))}
        {view.domainRects.map((r) => (
          <g
            key={r.node.id}
            data-testid={`domain-node-${r.node.id}`}
            onClick={() => r.flowCount > 0 && undefined}
          >
            <rect
              x={r.x}
              y={r.y}
              width={NODE_W}
              height={NODE_H}
              rx={8}
              fill="var(--color-surface)"
              stroke="var(--color-border-medium)"
              strokeWidth={1}
            />
            <text
              x={r.x + 12}
              y={r.y + 22}
              fontSize={13}
              fontWeight={600}
              fill="currentColor"
            >
              {r.node.name}
            </text>
            <text
              x={r.x + 12}
              y={r.y + 42}
              fontSize={11}
              fill="var(--color-text-muted)"
            >
              {r.flowCount} flow{r.flowCount === 1 ? "" : "s"}
              {r.node.domainMeta?.entryType
                ? ` · ${r.node.domainMeta.entryType}`
                : ""}
            </text>
          </g>
        ))}
      </svg>
    )
  }

  // detail: flows + steps
  return (
    <svg
      width={view.width}
      height={view.height}
      viewBox={`0 0 ${view.width} ${view.height}`}
      data-testid="domain-detail-svg"
    >
      <defs>
        <marker
          id="fs-arrow"
          viewBox="0 0 10 10"
          refX="9"
          refY="5"
          markerWidth="6"
          markerHeight="6"
          orient="auto"
        >
          <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--color-border-medium)" />
        </marker>
      </defs>
      {view.flowStepEdges.map((e, i) => (
        <line
          key={`fs-${i}`}
          x1={e.fromX}
          y1={e.fromY}
          x2={e.toX}
          y2={e.toY}
          stroke="var(--color-border-medium)"
          strokeWidth={1.5}
          markerEnd="url(#fs-arrow)"
        />
      ))}
      {view.flowRects.map((r) => (
        <g key={r.node.id} data-testid={`flow-node-${r.node.id}`}>
          <rect
            x={r.x}
            y={r.y}
            width={NODE_W}
            height={NODE_H}
            rx={8}
            fill="var(--color-elevated)"
            stroke="var(--color-border-medium)"
            strokeWidth={1}
          />
          <text
            x={r.x + 12}
            y={r.y + 22}
            fontSize={13}
            fontWeight={600}
            fill="currentColor"
          >
            {r.node.name}
          </text>
          <text
            x={r.x + 12}
            y={r.y + 42}
            fontSize={11}
            fill="var(--color-text-muted)"
          >
            {r.stepCount} step{r.stepCount === 1 ? "" : "s"}
            {r.node.domainMeta?.entryType
              ? ` · ${r.node.domainMeta.entryType}`
              : ""}
          </text>
        </g>
      ))}
      {view.stepRects.map((r) => (
        <g key={r.node.id} data-testid={`step-node-${r.node.id}`}>
          <rect
            x={r.x}
            y={r.y}
            width={STEP_W}
            height={STEP_H}
            rx={6}
            fill="var(--color-surface)"
            stroke="var(--color-border-subtle)"
            strokeWidth={1}
          />
          <text
            x={r.x + 10}
            y={r.y + 18}
            fontSize={11}
            fontWeight={500}
            fill="currentColor"
          >
            {truncate(r.node.name, 26)}
          </text>
          <text
            x={r.x + 10}
            y={r.y + 36}
            fontSize={10}
            fill="var(--color-text-muted)"
          >
            step {r.order}
          </text>
        </g>
      ))}
    </svg>
  )
}

function truncate(s: string, n: number): string {
  if (s.length <= n) return s
  return `${s.slice(0, n - 1)}…`
}