// P3-A: Missing-edge suggestions from the graph reviewer.
//
// The deterministic reviewer in `code_wiki_reviewer.rs` validates
// structural integrity (no dangling refs, no duplicate ids, etc.)
// but it doesn't *predict* missing edges. UA's `graph-reviewer`
// agent raises rules like "Service nodes should have at least one
// deploys/depends_on edge — warn if missing".
//
// We replicate 10 such rules here. Output is a list of
// `MissingEdgeSuggestion` entries that the dashboard can surface
// in a "Missing Edges" panel — actionable hints about which edges
// the LLM (or the deterministic extractors) likely missed.
//
// All functions are pure: input is the in-memory `KnowledgeGraph`,
// output is `Vec<MissingEdgeSuggestion>`. The reviewer in
// `code_wiki_reviewer.rs` calls `detect_missing_edges` and tacks
// the result onto `ReviewReport`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::commands::code_wiki_pipeline::KnowledgeGraph;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingEdgeSuggestion {
    /// Stable rule id, e.g. "service-needs-deploys-or-depends".
    pub rule_id: String,
    /// Node id that violates the rule.
    pub node_id: String,
    /// File path (for UI navigation).
    pub file_path: String,
    /// Suggested edge kind.
    pub edge_kind: String,
    /// Suggested target node id (None = unknown; UI offers
    /// a "let me pick" prompt instead).
    #[serde(default)]
    pub suggested_target: Option<String>,
    /// "error" | "warning" | "info".
    pub severity: String,
    /// Human-readable description shown in the panel.
    pub description: String,
}

/// Run all 10 missing-edge rules over the graph. Order is
/// deterministic so the dashboard can show stable lists between
/// re-runs of the same graph.
pub fn detect_missing_edges(graph: &KnowledgeGraph) -> Vec<MissingEdgeSuggestion> {
    let mut out = Vec::new();

    // Pre-compute helper sets so the rule functions stay O(N+E)
    // rather than O(N²).
    let out_edge_kinds_by_source: std::collections::HashMap<&str, HashSet<&str>> = {
        let mut m: std::collections::HashMap<&str, HashSet<&str>> =
            std::collections::HashMap::new();
        for e in &graph.edges {
            m.entry(e.source.as_str())
                .or_default()
                .insert(e.kind.as_str());
        }
        m
    };
    let in_edge_count_by_target: std::collections::HashMap<&str, u32> = {
        let mut m: std::collections::HashMap<&str, u32> =
            std::collections::HashMap::new();
        for e in &graph.edges {
            *m.entry(e.target.as_str()).or_insert(0) += 1;
        }
        m
    };

    for node in &graph.nodes {
        let node_edges = out_edge_kinds_by_source
            .get(node.id.as_str())
            .cloned()
            .unwrap_or_default();
        let inbound_count = in_edge_count_by_target
            .get(node.id.as_str())
            .copied()
            .unwrap_or(0);

        // Rule 1: service → deploys / depends_on
        if node.kind == "service"
            && !(node_edges.contains("deploys") || node_edges.contains("depends_on"))
        {
            out.push(MissingEdgeSuggestion {
                rule_id: "service-needs-deploys-or-depends".to_string(),
                node_id: node.id.clone(),
                file_path: node.file_path.clone(),
                edge_kind: "deploys".to_string(),
                suggested_target: None,
                severity: "warning".to_string(),
                description: format!(
                    "Service '{}' has no deploys or depends_on edge — likely missing infrastructure relationship.",
                    node.name
                ),
            });
        }

        // Rule 2: table → migrates / defines_schema
        if node.kind == "table"
            && !(node_edges.contains("migrates") || node_edges.contains("defines_schema"))
        {
            out.push(MissingEdgeSuggestion {
                rule_id: "table-needs-migrates-or-defines-schema".to_string(),
                node_id: node.id.clone(),
                file_path: node.file_path.clone(),
                edge_kind: "migrates".to_string(),
                suggested_target: None,
                severity: "warning".to_string(),
                description: format!(
                    "Table '{}' has no migrates or defines_schema edge — schema/origin is unclear.",
                    node.name
                ),
            });
        }

        // Rule 3: schema → defines_schema
        if node.kind == "schema" && !node_edges.contains("defines_schema") {
            out.push(MissingEdgeSuggestion {
                rule_id: "schema-needs-defines-schema".to_string(),
                node_id: node.id.clone(),
                file_path: node.file_path.clone(),
                edge_kind: "defines_schema".to_string(),
                suggested_target: None,
                severity: "warning".to_string(),
                description: format!(
                    "Schema '{}' has no defines_schema edge — what does it describe?",
                    node.name
                ),
            });
        }

        // Rule 4: pipeline → triggers
        if node.kind == "pipeline" && !node_edges.contains("triggers") {
            out.push(MissingEdgeSuggestion {
                rule_id: "pipeline-needs-triggers".to_string(),
                node_id: node.id.clone(),
                file_path: node.file_path.clone(),
                edge_kind: "triggers".to_string(),
                suggested_target: None,
                severity: "warning".to_string(),
                description: format!(
                    "Pipeline '{}' has no triggers edge — what does it activate?",
                    node.name
                ),
            });
        }

        // Rule 5: route file → routes
        if node.kind == "file"
            && is_route_path(&node.file_path)
            && !node_edges.contains("routes")
        {
            out.push(MissingEdgeSuggestion {
                rule_id: "route-needs-routes".to_string(),
                node_id: node.id.clone(),
                file_path: node.file_path.clone(),
                edge_kind: "routes".to_string(),
                suggested_target: None,
                severity: "info".to_string(),
                description: format!(
                    "Route file '{}' has no routes edge — which service does it dispatch to?",
                    node.file_path
                ),
            });
        }

        // Rule 6: config file → configures
        if node.kind == "file"
            && node.file_path.contains("tsconfig")
                || node.file_path.ends_with("package.json")
                || node.file_path.ends_with("Cargo.toml")
                || node.file_path.contains(".env")
        {
            // already covered by P1-A; only fire if truly missing
            if !node_edges.contains("configures") {
                out.push(MissingEdgeSuggestion {
                    rule_id: "config-needs-configures".to_string(),
                    node_id: node.id.clone(),
                    file_path: node.file_path.clone(),
                    edge_kind: "configures".to_string(),
                    suggested_target: None,
                    severity: "info".to_string(),
                    description: format!(
                        "Config file '{}' has no configures edge — what does it configure?",
                        node.file_path
                    ),
                });
            }
        }

        // Rule 7: complex function without any calls
        if node.kind == "function"
            && node.complexity == "complex"
            && graph
                .edges
                .iter()
                .filter(|e| e.source == node.id && e.kind == "calls")
                .count()
                == 0
        {
            out.push(MissingEdgeSuggestion {
                rule_id: "function-complex-needs-calls".to_string(),
                node_id: node.id.clone(),
                file_path: node.file_path.clone(),
                edge_kind: "calls".to_string(),
                suggested_target: None,
                severity: "info".to_string(),
                description: format!(
                    "Complex function '{}' has no outbound calls edge — is it self-contained?",
                    node.name
                ),
            });
        }

        // Rule 8: file without contains edge (but has function/class children)
        if node.kind == "file" && !node_edges.contains("contains") {
            let has_children = graph
                .nodes
                .iter()
                .any(|n| n.file_path == node.file_path && (n.kind == "function" || n.kind == "class"));
            if has_children {
                out.push(MissingEdgeSuggestion {
                    rule_id: "file-needs-contains-function".to_string(),
                    node_id: node.id.clone(),
                    file_path: node.file_path.clone(),
                    edge_kind: "contains".to_string(),
                    suggested_target: None,
                    severity: "warning".to_string(),
                    description: format!(
                        "File '{}' has function/class nodes but no contains edge.",
                        node.file_path
                    ),
                });
            }
        }

        // Rule 9: isolated module (no inbound edges)
        if node.kind == "module" && inbound_count == 0 {
            out.push(MissingEdgeSuggestion {
                rule_id: "isolated-module".to_string(),
                node_id: node.id.clone(),
                file_path: node.file_path.clone(),
                edge_kind: "imports".to_string(),
                suggested_target: None,
                severity: "warning".to_string(),
                description: format!(
                    "Module '{}' has no inbound edges — orphan in the graph.",
                    node.name
                ),
            });
        }

        // Rule 10: domain flow without inbound contains_flow
        if node.kind == "flow" && inbound_count == 0 {
            out.push(MissingEdgeSuggestion {
                rule_id: "orphan-flow-needs-contains-flow".to_string(),
                node_id: node.id.clone(),
                file_path: node.file_path.clone(),
                edge_kind: "contains_flow".to_string(),
                suggested_target: None,
                severity: "info".to_string(),
                description: format!(
                    "Flow '{}' has no inbound contains_flow edge — which domain owns it?",
                    node.name
                ),
            });
        }
    }

    out
}

fn is_route_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/routes/")
        || lower.contains("/router/")
        || lower.ends_with("routes.ts")
        || lower.ends_with("router.ts")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::{GraphEdge, GraphNode, ProjectMeta};

    fn make_node(id: &str, kind: &str, file_path: &str, complexity: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            kind: kind.to_string(),
            name: id.to_string(),
            file_path: file_path.to_string(),
            summary: String::new(),
            tags: vec![],
            complexity: complexity.to_string(),
            location: None,
            language_notes: None,
        }
    }

    fn make_edge(source: &str, target: &str, kind: &str) -> GraphEdge {
        GraphEdge {
            source: source.to_string(),
            target: target.to_string(),
            kind: kind.to_string(),
            direction: "forward".to_string(),
            weight: 1.0,
            description: None,
        }
    }

    fn make_graph(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> KnowledgeGraph {
        KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: "test".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: String::new(),
                analyzed_at: "2026-01-01".to_string(),
                git_commit_hash: String::new(),
            },
            nodes,
            edges,
            layers: vec![],
            tour: vec![],
        }
    }

    #[test]
    fn service_without_deploys_triggers_suggestion() {
        let g = make_graph(
            vec![make_node("service:auth", "service", "src/auth.ts", "moderate")],
            vec![],
        );
        let suggestions = detect_missing_edges(&g);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].rule_id, "service-needs-deploys-or-depends");
        assert_eq!(suggestions[0].edge_kind, "deploys");
        assert_eq!(suggestions[0].severity, "warning");
    }

    #[test]
    fn service_with_deploys_no_suggestion() {
        let g = make_graph(
            vec![make_node("service:auth", "service", "src/auth.ts", "moderate")],
            vec![make_edge("service:auth", "infra:k8s", "deploys")],
        );
        let suggestions = detect_missing_edges(&g);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn table_without_migrates_triggers_suggestion() {
        let g = make_graph(
            vec![make_node("table:users", "table", "models/user.ts", "moderate")],
            vec![],
        );
        let suggestions = detect_missing_edges(&g);
        assert!(suggestions.iter().any(|s| s.rule_id == "table-needs-migrates-or-defines-schema"));
    }

    #[test]
    fn schema_without_defines_schema_triggers_suggestion() {
        let g = make_graph(
            vec![make_node("schema:user", "schema", "schema/user.graphql", "moderate")],
            vec![],
        );
        let suggestions = detect_missing_edges(&g);
        assert!(suggestions.iter().any(|s| s.rule_id == "schema-needs-defines-schema"));
    }

    #[test]
    fn function_complex_without_calls_triggers_info() {
        let g = make_graph(
            vec![make_node(
                "function:src/api.ts:handler",
                "function",
                "src/api.ts",
                "complex",
            )],
            vec![],
        );
        let suggestions = detect_missing_edges(&g);
        assert!(suggestions.iter().any(|s| s.rule_id == "function-complex-needs-calls"));
        let s = suggestions
            .iter()
            .find(|s| s.rule_id == "function-complex-needs-calls")
            .unwrap();
        assert_eq!(s.severity, "info");
    }

    #[test]
    fn isolated_module_triggers_suggestion() {
        let g = make_graph(
            vec![make_node("module:orphan", "module", "src/orphan.ts", "moderate")],
            vec![],
        );
        let suggestions = detect_missing_edges(&g);
        assert!(suggestions.iter().any(|s| s.rule_id == "isolated-module"));
    }

    #[test]
    fn route_file_without_routes_triggers_info() {
        let g = make_graph(
            vec![make_node("file:src/routes/index.ts", "file", "src/routes/index.ts", "moderate")],
            vec![],
        );
        let suggestions = detect_missing_edges(&g);
        assert!(suggestions.iter().any(|s| s.rule_id == "route-needs-routes"));
    }

    #[test]
    fn file_with_function_children_without_contains_triggers_warning() {
        let g = make_graph(
            vec![
                make_node("file:src/lib.ts", "file", "src/lib.ts", "moderate"),
                make_node("function:src/lib.ts:foo", "function", "src/lib.ts", "moderate"),
            ],
            // No contains edge
            vec![],
        );
        let suggestions = detect_missing_edges(&g);
        assert!(suggestions
            .iter()
            .any(|s| s.rule_id == "file-needs-contains-function"));
    }

    #[test]
    fn empty_graph_yields_no_suggestions() {
        let g = make_graph(vec![], vec![]);
        let suggestions = detect_missing_edges(&g);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn suggestions_are_deterministic() {
        let g = make_graph(
            vec![
                make_node("service:a", "service", "src/a.ts", "moderate"),
                make_node("table:b", "table", "src/b.ts", "moderate"),
                make_node("schema:c", "schema", "src/c.ts", "moderate"),
            ],
            vec![],
        );
        let s1 = detect_missing_edges(&g);
        let s2 = detect_missing_edges(&g);
        assert_eq!(s1.len(), s2.len());
        for (a, b) in s1.iter().zip(s2.iter()) {
            assert_eq!(a.rule_id, b.rule_id);
            assert_eq!(a.node_id, b.node_id);
        }
    }
}