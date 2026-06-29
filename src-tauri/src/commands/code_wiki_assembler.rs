// Phase 3 — ASSEMBLE. Mirrors UA's `merge-batch-graphs.py` but
// works on the in-memory graph (Phase 2 already produced a single
// graph, not separate batch outputs). The steps are:
//   1. Normalize node IDs (strip duplicates of the type prefix
//      like "file:file:src/main.ts" -> "file:src/main.ts").
//   2. Normalize complexity values (low -> simple, etc.).
//   3. Rewrite edge source/target to match corrected node IDs.
//   4. Deduplicate nodes by ID (keep last occurrence; later
//      enrichments win).
//   5. Deduplicate edges by (source, target, type).
//   6. Drop edges whose source or target no longer exists.
//   7. Compute stats: totalNodes, totalEdges, byLanguage,
//      byNodeType.
// Returns a small `AssembleReport` describing what changed so
// the pipeline can surface it as a warning.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;

use crate::commands::code_wiki_pipeline::{GraphEdge, GraphNode, KnowledgeGraph};

#[derive(Debug, Clone, Serialize, Default)]
pub struct AssembleReport {
    pub nodes_renamed: u32,
    pub nodes_deduped: u32,
    pub edges_deduped: u32,
    pub edges_dropped: u32,
    pub complexity_normalized: u32,
    pub by_language: BTreeMap<String, u32>,
    pub by_node_type: BTreeMap<String, u32>,
    pub nodes_total: u32,
    pub edges_total: u32,
}

/// Run the assembly pass. Returns a (graph, report) tuple.
/// See module docs for the steps.
pub fn assemble(mut graph: KnowledgeGraph) -> (KnowledgeGraph, AssembleReport) {
    let mut report = AssembleReport::default();

    // 1 + 4. Normalize IDs and dedupe nodes by id (later wins).
    graph.nodes = {
        let mut by_id: HashMap<String, GraphNode> = HashMap::new();
        for node in graph.nodes {
            let mut n = node;
            let normalised = normalise_node_id(&n.id, &n.kind);
            if normalised != n.id {
                report.nodes_renamed += 1;
            }
            n.id = normalised;
            if by_id.contains_key(&n.id) {
                report.nodes_deduped += 1;
            }
            by_id.insert(n.id.clone(), n);
        }
        // Stable ordering: sort by id for deterministic output.
        let mut v: Vec<GraphNode> = by_id.into_values().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    };

    // 2. Normalize complexity values.
    for node in graph.nodes.iter_mut() {
        let normalised = normalise_complexity(&node.complexity);
        if normalised != node.complexity {
            report.complexity_normalized += 1;
        }
        node.complexity = normalised;
    }

    // 3 + 6. Build node-id set, dedupe edges, drop dangling.
    let valid_ids: HashSet<String> = graph.nodes.iter().map(|n| n.id.clone()).collect();
    let mut by_key: HashMap<(String, String, String), GraphEdge> = HashMap::new();
    for edge in graph.edges {
        let source = edge.source.clone();
        let target = edge.target.clone();
        let kind = edge.kind.clone();
        if !valid_ids.contains(&source) || !valid_ids.contains(&target) {
            report.edges_dropped += 1;
            continue;
        }
        let key = (source, target, kind);
        if by_key.contains_key(&key) {
            report.edges_deduped += 1;
        } else {
            by_key.insert(key, edge);
        }
    }
    let mut edges: Vec<GraphEdge> = by_key.into_values().collect();
    edges.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.target.cmp(&b.target))
            .then(a.kind.cmp(&b.kind))
    });
    graph.edges = edges;

    // 7. Compute stats.
    for node in &graph.nodes {
        *report
            .by_node_type
            .entry(node.kind.clone())
            .or_insert(0) += 1;
        if let Some(lang) = &node.language_notes {
            *report
                .by_language
                .entry(lang.clone())
                .or_insert(0) += 1;
        }
    }
    report.nodes_total = graph.nodes.len() as u32;
    report.edges_total = graph.edges.len() as u32;

    (graph, report)
}

fn normalise_node_id(id: &str, _kind: &str) -> String {
    // Strip a duplicate type prefix. Example:
    //   "file:file:src/main.ts" -> "file:src/main.ts"
    //   "function:function:src/lib.rs:foo" -> "function:src/lib.rs:foo"
    // We detect by finding the FIRST ':' in the id; the prefix is
    // everything before it. If the same prefix appears again
    // immediately after, strip one.
    let Some(colon) = id.find(':') else { return id.to_string() };
    let prefix = &id[..colon];
    let rest = &id[colon + 1..];
    if rest.starts_with(prefix) && rest[prefix.len()..].starts_with(':') {
        // Strip the duplicate prefix and the second colon.
        return format!("{}:{}", prefix, &rest[prefix.len() + 1..]);
    }
    id.to_string()
}

fn normalise_complexity(s: &str) -> String {
    // UA's normalisation table. Anything not in the table maps to
    // "moderate" as a safe default.
    match s.to_ascii_lowercase().as_str() {
        "low" | "simple" | "trivial" | "easy" => "simple".to_string(),
        "medium" | "moderate" | "normal" | "med" => "moderate".to_string(),
        "high" | "complex" | "hard" | "difficult" => "complex".to_string(),
        _ => "moderate".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::ProjectMeta;

    fn n(id: &str, kind: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            kind: kind.to_string(),
            name: "x".to_string(),
            file_path: String::new(),
            summary: String::new(),
            tags: vec![],
            complexity: "moderate".to_string(),
            location: None,
            language_notes: None,
        }
    }

    fn e(source: &str, target: &str, kind: &str) -> GraphEdge {
        GraphEdge {
            source: source.to_string(),
            target: target.to_string(),
            kind: kind.to_string(),
            direction: "forward".to_string(),
            weight: 1.0,
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
    fn strips_duplicate_type_prefix() {
        assert_eq!(normalise_node_id("file:file:src/main.ts", "file"), "file:src/main.ts");
        assert_eq!(normalise_node_id("function:function:src/lib.rs:foo", "function"), "function:src/lib.rs:foo");
    }

    #[test]
    fn leaves_correctly_prefixed_ids_alone() {
        assert_eq!(normalise_node_id("file:src/main.ts", "file"), "file:src/main.ts");
        assert_eq!(normalise_node_id("class:src/lib.rs:Foo", "class"), "class:src/lib.rs:Foo");
    }

    #[test]
    fn normalises_complexity_aliases() {
        assert_eq!(normalise_complexity("low"), "simple");
        assert_eq!(normalise_complexity("MEDIUM"), "moderate");
        assert_eq!(normalise_complexity("high"), "complex");
        assert_eq!(normalise_complexity("garbage"), "moderate");
    }

    #[test]
    fn drops_dangling_edges() {
        let graph = g(
            vec![n("file:a.ts", "file")],
            vec![e("file:a.ts", "file:missing.ts", "contains")],
        );
        let (out, report) = assemble(graph);
        assert_eq!(out.edges.len(), 0);
        assert_eq!(report.edges_dropped, 1);
    }

    #[test]
    fn dedupes_edges_by_triple() {
        let graph = g(
            vec![n("file:a.ts", "file"), n("file:b.ts", "file")],
            vec![
                e("file:a.ts", "file:b.ts", "contains"),
                e("file:a.ts", "file:b.ts", "contains"),
            ],
        );
        let (out, report) = assemble(graph);
        assert_eq!(out.edges.len(), 1);
        assert_eq!(report.edges_deduped, 1);
    }

    #[test]
    fn dedupes_nodes_by_id() {
        let graph = g(
            vec![
                n("file:a.ts", "file"),
                n("file:a.ts", "file"),
                n("file:b.ts", "file"),
            ],
            vec![],
        );
        let (out, report) = assemble(graph);
        assert_eq!(out.nodes.len(), 2);
        assert_eq!(report.nodes_deduped, 1);
    }

    #[test]
    fn renames_duplicate_prefix() {
        let graph = g(
            vec![n("file:file:src/main.ts", "file")],
            vec![],
        );
        let (out, report) = assemble(graph);
        assert_eq!(out.nodes[0].id, "file:src/main.ts");
        assert_eq!(report.nodes_renamed, 1);
    }

    #[test]
    fn produces_deterministic_node_order() {
        let graph = g(
            vec![
                n("file:z.ts", "file"),
                n("file:a.ts", "file"),
                n("file:m.ts", "file"),
            ],
            vec![],
        );
        let (out, _) = assemble(graph);
        let ids: Vec<&str> = out.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["file:a.ts", "file:m.ts", "file:z.ts"]);
    }

    #[test]
    fn computes_stats_by_node_type() {
        let graph = g(
            vec![
                n("file:a.ts", "file"),
                n("file:b.ts", "file"),
                n("function:a.ts:foo", "function"),
            ],
            vec![],
        );
        let (_, report) = assemble(graph);
        assert_eq!(report.by_node_type.get("file").copied(), Some(2));
        assert_eq!(report.by_node_type.get("function").copied(), Some(1));
        assert_eq!(report.nodes_total, 3);
    }
}
