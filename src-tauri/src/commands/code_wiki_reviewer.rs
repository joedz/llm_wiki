// Phase 6 — REVIEW. Mirrors UA's inline deterministic
// validation (the `ua-inline-validate.cjs` script). For M3 we
// skip the LLM `--review` mode; a future follow-up can add the
// LLM-based review (UA's `graph-reviewer` agent).
//
// The validation checks every node + edge + layer + tour entry
// for completeness and consistency, records any issues as
// warnings, and computes a small `GraphStats` block the dashboard
// can surface.

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::commands::code_wiki_architecture::Layer;
use crate::commands::code_wiki_pipeline::KnowledgeGraph;
use crate::commands::code_wiki_tour::TourStep;

#[derive(Debug, Clone, Serialize)]
pub struct ReviewIssue {
    pub level: String,   // "error" | "warning"
    pub category: String,
    pub message: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct GraphStats {
    pub total_nodes: u32,
    pub total_edges: u32,
    pub total_layers: u32,
    pub tour_steps: u32,
    pub by_node_type: BTreeMap<String, u32>,
    pub by_edge_type: BTreeMap<String, u32>,
    pub orphan_nodes: u32,
    pub languages: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewReport {
    pub issues: Vec<ReviewIssue>,
    pub stats: GraphStats,
    /// P3-A: actionable hints about which edges the graph is
    /// likely missing. Surfaced in the dashboard's "Missing Edges"
    /// panel.
    #[serde(default)]
    pub missing_edges: Vec<crate::commands::code_wiki_missing_edges::MissingEdgeSuggestion>,
}

/// Run the inline validation. Always returns OK — issues are
/// surfaced as warnings (UA's "always save partial" rule). The
/// caller can decide whether to fail the pipeline based on the
/// issue count.
pub fn review_graph(
    graph: &KnowledgeGraph,
    layers: &[Layer],
    tour: &[TourStep],
) -> ReviewReport {
    let mut issues: Vec<ReviewIssue> = Vec::new();
    let mut stats = GraphStats::default();

    let mut node_ids: HashSet<String> = HashSet::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    for (i, n) in graph.nodes.iter().enumerate() {
        if n.id.is_empty() {
            issues.push(ReviewIssue {
                level: "error".to_string(),
                category: "node".to_string(),
                message: format!("Node[{i}] missing id"),
                path: None,
            });
            continue;
        }
        if n.name.is_empty() {
            issues.push(ReviewIssue {
                level: "warning".to_string(),
                category: "node".to_string(),
                message: format!("Node '{}' missing name", n.id),
                path: Some(n.id.clone()),
            });
        }
        // NOTE: codegraph doesn't extract docstrings for Python/many compiled
        // languages, so summary is often empty — skip silently.
        if !seen_ids.insert(n.id.clone()) {
            issues.push(ReviewIssue {
                level: "error".to_string(),
                category: "node".to_string(),
                message: format!("Duplicate node id '{}'", n.id),
                path: Some(n.id.clone()),
            });
        }
        node_ids.insert(n.id.clone());
    }

    for e in &graph.edges {
        if !node_ids.contains(&e.source) {
            issues.push(ReviewIssue {
                level: "error".to_string(),
                category: "edge".to_string(),
                message: format!("Edge source '{}' not found", e.source),
                path: Some(e.source.clone()),
            });
        }
        if !node_ids.contains(&e.target) {
            issues.push(ReviewIssue {
                level: "error".to_string(),
                category: "edge".to_string(),
                message: format!("Edge target '{}' not found", e.target),
                path: Some(e.target.clone()),
            });
        }
    }

    // Layer validation: every nodeIds entry must exist, no
    // duplicates across layers.
    let mut assigned: HashSet<String> = HashSet::new();
    for layer in layers {
        for nid in &layer.node_ids {
            if !node_ids.contains(nid) {
                issues.push(ReviewIssue {
                    level: "error".to_string(),
                    category: "layer".to_string(),
                    message: format!("Layer '{}' refs missing node '{}'", layer.id, nid),
                    path: Some(layer.id.clone()),
                });
            }
            if !assigned.insert(nid.clone()) {
                issues.push(ReviewIssue {
                    level: "warning".to_string(),
                    category: "layer".to_string(),
                    message: format!("Node '{}' appears in multiple layers", nid),
                    path: Some(nid.clone()),
                });
            }
        }
    }
    // File-level nodes should be in a layer (UA invariant).
    for n in &graph.nodes {
        let is_file_level = matches!(
            n.kind.as_str(),
            "file" | "config" | "document" | "service" | "pipeline"
                | "table" | "schema" | "resource" | "endpoint"
        );
        if is_file_level && !assigned.contains(&n.id) {
            issues.push(ReviewIssue {
                level: "info".to_string(),
                category: "layer".to_string(),
                message: format!("File node '{}' not in any layer", n.id),
                path: Some(n.id.clone()),
            });
        }
    }

    // Tour validation: every nodeIds entry must exist.
    for (i, step) in tour.iter().enumerate() {
        for nid in &step.node_ids {
            if !node_ids.contains(nid) {
                issues.push(ReviewIssue {
                    level: "error".to_string(),
                    category: "tour".to_string(),
                    message: format!("Tour step[{i}] refs missing node '{}'", nid),
                    path: Some(step.title.clone()),
                });
            }
        }
    }

    // Stats: counts, by-type histograms, orphan detection.
    let connected: HashSet<String> = graph
        .edges
        .iter()
        .flat_map(|e| [e.source.clone(), e.target.clone()])
        .collect();
    for n in &graph.nodes {
        if !connected.contains(&n.id) {
            stats.orphan_nodes += 1;
        }
        *stats.by_node_type.entry(n.kind.clone()).or_insert(0) += 1;
        if let Some(lang) = &n.language_notes {
            *stats.languages.entry(lang.clone()).or_insert(0) += 1;
        }
    }
    for e in &graph.edges {
        *stats.by_edge_type.entry(e.kind.clone()).or_insert(0) += 1;
    }
    stats.total_nodes = graph.nodes.len() as u32;
    stats.total_edges = graph.edges.len() as u32;
    stats.total_layers = layers.len() as u32;
    stats.tour_steps = tour.len() as u32;

    // P3-A: detect missing edges via the rules module.
    let missing_edges =
        crate::commands::code_wiki_missing_edges::detect_missing_edges(graph);

    ReviewReport {
        issues,
        stats,
        missing_edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::{GraphEdge, GraphNode, ProjectMeta};

    fn n(id: &str, kind: &str, summary: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            kind: kind.to_string(),
            name: id.to_string(),
            file_path: id.to_string(),
            summary: summary.to_string(),
            tags: vec![],
            complexity: "moderate".to_string(),
            location: None,
            language_notes: None,
        }
    }

    fn e(source: &str, target: &str) -> GraphEdge {
        GraphEdge {
            source: source.to_string(),
            target: target.to_string(),
            kind: "contains".to_string(),
            direction: "forward".to_string(),
            weight: 1.0,
            ..Default::default()
        }
    }

    fn g(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> KnowledgeGraph {
        KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: String::new(),
                analyzed_at: String::new(),
                git_commit_hash: String::new(),
            },
            nodes,
            edges,
            layers: vec![],
            tour: vec![],
        }
    }

    #[test]
    fn clean_graph_has_no_issues() {
        let g = g(
            vec![n("file:a.ts", "file", "A")],
            vec![],
        );
        let report = review_graph(&g, &[], &[]);
        let errors: Vec<&ReviewIssue> = report.issues.iter().filter(|i| i.level == "error").collect();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert_eq!(report.stats.total_nodes, 1);
        assert_eq!(report.stats.by_node_type.get("file").copied(), Some(1));
    }

    #[test]
    fn detects_dangling_edge() {
        let g = g(
            vec![n("file:a.ts", "file", "A")],
            vec![e("file:a.ts", "file:missing.ts")],
        );
        let report = review_graph(&g, &[], &[]);
        let dangling: Vec<&ReviewIssue> = report.issues.iter()
            .filter(|i| i.category == "edge" && i.message.contains("not found"))
            .collect();
        assert_eq!(dangling.len(), 1);
    }

    #[test]
    fn detects_duplicate_node_id() {
        let g = g(
            vec![n("file:a.ts", "file", "A"), n("file:a.ts", "file", "A2")],
            vec![],
        );
        let report = review_graph(&g, &[], &[]);
        let dups: Vec<&ReviewIssue> = report.issues.iter()
            .filter(|i| i.message.contains("Duplicate node id"))
            .collect();
        assert_eq!(dups.len(), 1);
    }

    #[test]
    fn counts_orphan_nodes() {
        let g = g(
            vec![
                n("file:a.ts", "file", "A"),
                n("file:orphan.ts", "file", "B"),  // no edges
                n("file:b.ts", "file", "C"),
            ],
            vec![e("file:a.ts", "file:b.ts")],
        );
        let report = review_graph(&g, &[], &[]);
        assert_eq!(report.stats.orphan_nodes, 1);
    }

    #[test]
    fn computes_by_type_histograms() {
        let g = g(
            vec![
                n("file:a.ts", "file", "A"),
                n("file:b.ts", "file", "B"),
                n("function:a.ts:foo", "function", "foo"),
            ],
            vec![e("file:a.ts", "function:a.ts:foo")],
        );
        let report = review_graph(&g, &[], &[]);
        assert_eq!(report.stats.by_node_type.get("file").copied(), Some(2));
        assert_eq!(report.stats.by_node_type.get("function").copied(), Some(1));
        assert_eq!(report.stats.by_edge_type.get("contains").copied(), Some(1));
    }

    #[test]
    fn layer_missing_node_is_error() {
        let g = g(vec![n("file:a.ts", "file", "A")], vec![]);
        let layers = vec![Layer {
            id: "layer:api".to_string(),
            name: "API".to_string(),
            description: "API".to_string(),
            node_ids: vec!["file:missing.ts".to_string()],
        }];
        let report = review_graph(&g, &layers, &[]);
        let err = report.issues.iter().find(|i| i.category == "layer" && i.message.contains("missing node"));
        assert!(err.is_some());
    }
}
