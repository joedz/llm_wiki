// P3-C: Domain view via Sigma.js + ForceAtlas2.
//
// Replaces the hand-rolled SVG renderer (which had no drag / zoom /
// hover interactions) with a SigmaContainer + graphology graph
// + forceAtlas2 layout. Reuses the existing @react-sigma/core and
// graphology-layout-forceatlas2 dependencies (already in
// package.json for the main `GraphView`).
//
// Two modes:
//   - Overview (default): all domain nodes + cross_domain edges
//   - Detail (after click): the chosen domain's flows + their steps

import { useEffect, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { Loader2, Network, X, ArrowLeft } from "lucide-react"
import Graph from "graphology"
import forceAtlas2 from "graphology-layout-forceatlas2"
import { SigmaContainer, useLoadGraph, useRegisterEvents } from "@react-sigma/core"
import "@react-sigma/core/lib/style.css"
import {
  getDomainGraph,
  type DomainGraph as DomainGraphT,
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

const NODE_COLOR: Record<string, string> = {
  domain: "#a78bfa",  // violet-400
  flow: "#60a5fa",    // blue-400
  step: "#34d399",    // emerald-400
}

const NODE_SIZE: Record<string, number> = {
  domain: 24,
  flow: 16,
  step: 10,
}

export function DomainView({ open, projectPath, repoName, onClose }: Props) {
  const { t } = useTranslation()
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [graph, setGraph] = useState<DomainGraphT | null>(null)
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
          setGraph(g as DomainGraphT)
        }
      } catch (e) {
        setError(String(e))
      } finally {
        setLoading(false)
      }
    })()
  }, [open, projectPath, repoName])

  const builtGraph = useMemo(() => {
    if (!graph) return null
    if (activeDomainId) {
      return buildDomainDetailGraph(graph, activeDomainId)
    }
    return buildDomainOverviewGraph(graph)
  }, [graph, activeDomainId])

  // Apply ForceAtlas2 once per graph instance.
  useEffect(() => {
    if (!builtGraph) return
    if (builtGraph.order === 0) return
    try {
      forceAtlas2.assign(builtGraph, {
        iterations: 100,
        settings: {
          gravity: 1.2,
          scalingRatio: 8,
          slowDown: 5,
          strongGravityMode: true,
        },
      })
    } catch (e) {
      console.warn("[domain-view] ForceAtlas2 failed:", e)
    }
  }, [builtGraph])

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-background/80 p-4"
      role="dialog"
      aria-modal="true"
      data-testid="domain-view"
    >
      <div className="flex h-[90vh] w-full max-w-5xl flex-col rounded-md border bg-card shadow-lg">
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

        <div className="flex-1 overflow-hidden">
          {loading && (
            <div className="flex items-center gap-2 p-4 text-xs text-muted-foreground">
              <Loader2 className="h-3 w-3 animate-spin" />
              {t("codeWiki.domainView.loading", "Loading domain graph…")}
            </div>
          )}
          {error && (
            <div className="p-4 text-xs text-red-500">{error}</div>
          )}
          {!loading && !error && !builtGraph && (
            <p className="p-4 text-xs text-muted-foreground">
              {t(
                "codeWiki.domainView.empty",
                "No domain graph available. Run the domain pipeline first.",
              )}
            </p>
          )}
          {!loading && !error && builtGraph && (
            <SigmaContainer
              style={{ width: "100%", height: "100%", background: "transparent" }}
              settings={{
                defaultNodeType: "circle",
                defaultEdgeType: "line",
                renderEdgeLabels: true,
                labelSize: 12,
                labelColor: { color: "#1f2937" },
                edgeLabelSize: 10,
                defaultEdgeColor: "#cbd5e1",
                stagePadding: 30,
                minCameraRatio: 0.3,
                maxCameraRatio: 3,
                enableEdgeClickEvents: false,
              }}
            >
              <GraphLoader
                graph={builtGraph}
                activeDomainId={activeDomainId}
                onNodeClick={(id) => {
                  if (!activeDomainId) {
                    // Clicking a domain in overview mode drills in
                    const node = graph?.nodes.find((n) => n.id === id)
                    if (node && node.type === "domain") {
                      setActiveDomainId(id)
                    }
                  }
                }}
              />
            </SigmaContainer>
          )}
        </div>

        {builtGraph && (
          <footer className="flex items-center justify-between border-t p-2 text-xs text-muted-foreground">
            <span>
              {builtGraph.order} nodes · {builtGraph.size} edges
            </span>
            {activeDomainId && (
              <span>
                {t("codeWiki.domainView.activeDomain", "Viewing domain")}:{" "}
                <code>{activeDomainId}</code>
              </span>
            )}
          </footer>
        )}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Graph builders (graphology.Graph instances)
// ---------------------------------------------------------------------------

function buildDomainOverviewGraph(domainGraph: DomainGraphT): Graph {
  const g = new Graph({ multi: true, type: "directed" });
  for (const node of domainGraph.nodes) {
    if (node.type !== "domain") continue;
    g.addNode(node.id, {
      label: node.name,
      color: NODE_COLOR.domain ?? "#a78bfa",
      size: NODE_SIZE.domain ?? 20,
      x: pseudoRandomX(node.id),
      y: pseudoRandomY(node.id),
      nodeType: "domain",
    });
  }
  for (const edge of domainGraph.edges) {
    if (edge.type !== "cross_domain") continue;
    if (!g.hasNode(edge.source) || !g.hasNode(edge.target)) continue;
    g.addEdgeWithKey(`${edge.source}->${edge.target}`, edge.source, edge.target, {
      label: edge.description ?? "",
      color: "#f59e0b",
      size: 2,
    });
  }
  return g;
}

function buildDomainDetailGraph(
  domainGraph: DomainGraphT,
  domainId: string,
): Graph {
  const g = new Graph({ multi: false, type: "directed" });
  // flows
  const flowIds = new Set(
    domainGraph.edges
      .filter((e) => e.type === "contains_flow" && e.source === domainId)
      .map((e) => e.target),
  );
  const flowNodes = domainGraph.nodes.filter((n) => flowIds.has(n.id));
  for (const node of flowNodes) {
    g.addNode(node.id, {
      label: node.name,
      color: NODE_COLOR.flow ?? "#60a5fa",
      size: NODE_SIZE.flow ?? 14,
      x: pseudoRandomX(node.id),
      y: pseudoRandomY(node.id),
      nodeType: "flow",
    });
  }
  // step edges
  const stepEdges = domainGraph.edges.filter(
    (e) => e.type === "flow_step" && flowIds.has(e.source),
  );
  const stepIds = new Set(stepEdges.map((e) => e.target));
  const stepNodes = domainGraph.nodes.filter((n) => stepIds.has(n.id));
  for (const node of stepNodes) {
    g.addNode(node.id, {
      label: node.name,
      color: NODE_COLOR.step ?? "#34d399",
      size: NODE_SIZE.step ?? 8,
      x: pseudoRandomX(node.id),
      y: pseudoRandomY(node.id),
      nodeType: "step",
    });
  }
  // contains_flow
  for (const e of domainGraph.edges) {
    if (e.type === "contains_flow" && e.source === domainId) {
      if (g.hasNode(e.source) && g.hasNode(e.target)) {
        g.addEdgeWithKey(`${e.source}->${e.target}`, e.source, e.target, {
          color: "#94a3b8",
        });
      }
    }
  }
  // flow_step
  for (const e of stepEdges) {
    if (g.hasNode(e.source) && g.hasNode(e.target)) {
      g.addEdgeWithKey(`${e.source}->${e.target}`, e.source, e.target, {
        color: "#64748b",
        size: 1,
      });
    }
  }
  return g;
}

// Deterministic-ish per-id pseudo positions so successive
// re-renders don't churn the layout. ForceAtlas2 refines them.
function pseudoRandomX(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) {
    h = (h * 31 + id.charCodeAt(i)) | 0;
  }
  return ((h & 0xffff) / 0xffff - 0.5) * 4;
}

function pseudoRandomY(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) {
    h = (h * 37 + id.charCodeAt(i)) | 0;
  }
  return (((h >> 16) & 0xffff) / 0xffff - 0.5) * 4;
}

// ---------------------------------------------------------------------------
// Inner Sigma loader
// ---------------------------------------------------------------------------

interface GraphLoaderProps {
  graph: Graph
  activeDomainId: string | null
  onNodeClick?: (id: string) => void
}

function GraphLoader({ graph, activeDomainId, onNodeClick }: GraphLoaderProps) {
  const loadGraph = useLoadGraph();
  const registerEvents = useRegisterEvents();

  useEffect(() => {
    const g = loadGraph();
    // Clear and reload
    g.clear();
    g.import(graph.export());
    // Force a refresh
    if (typeof (g as unknown as { refresh?: () => void }).refresh === "function") {
      (g as unknown as { refresh: () => void }).refresh();
    }
  }, [graph, loadGraph]);

  useEffect(() => {
    if (!onNodeClick) return;
    const unregister = registerEvents({
      clickNode: (event) => {
        onNodeClick(event.node);
      },
    });
    return () => unregister();
  }, [onNodeClick, registerEvents, activeDomainId]);

  return null;
}