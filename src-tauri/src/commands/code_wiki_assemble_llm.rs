// Phase 5.5 — LLM-assisted post-merge cleanup.
//
// UA's `/understand` Phase 3 dispatches an `assemble-reviewer`
// subagent that handles what `merge-batch-graphs.py` cannot:
// unknown node kinds, unknown complexity values, dropped
// dangling edges, and cross-batch edge gaps. We mirror that
// after our deterministic `assemble()` runs.
//
// 1. **Sanity-check fixed section**: log AssembleReport stats.
//    This is informational — UA treats pattern domination as a
//    signal of upstream LLM consistency issues, but we have no
//    per-batch lineage, so we just record the report.
//
// 2. **Investigate "could not fix"**:
//    - Find nodes whose `kind` is not in our canonical set
//      (we use a small set — see `KNOWN_NODE_KINDS`). Prompt
//      the LLM to suggest remappings. Apply via direct field
//      update + ID-prefix rewrite.
//    - Find nodes whose `complexity` is not in
//      `{simple, moderate, complex}`. The deterministic pass
//      already normalizes the most common aliases, but the LLM
//      catches anything exotic. Apply.
//
// 3. **Cross-batch edge gaps**: scan the `import_map` on the
//    ScanResult (each entry: file → list of imported files).
//    For every import, verify a corresponding `imports` edge
//    exists in the graph. If not, add it with weight 0.7.
//
// 4. **Apply fixes** in place + return an AssembleReviewReport
//    that the pipeline persists into `meta.json`.
//
// LLM failures fall back to no-op (deterministic state is
// preserved). The pipeline emits a warning instead of failing.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::commands::code_wiki_assembler::AssembleReport;
use crate::commands::code_wiki_pipeline::{
    GraphEdge, GraphNode, KnowledgeGraph, LlmRequestSpec,
};
use crate::commands::code_wiki_scanner::ScanResult;
use crate::llm_client::{call_llm, LlmRequest, LlmResponse};

const ASSEMBLE_REVIEWER_PROMPT: &str = include_str!("../prompts/assemble_reviewer.md");

const KNOWN_NODE_KINDS: &[&str] = &[
    "file", "function", "class", "interface", "enum", "struct", "module",
    "concept", "service", "endpoint", "config", "document", "table",
    "schema", "pipeline", "resource", "topic", "article", "entity",
    "claim", "source", "domain", "flow", "step",
];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AssembleReviewReport {
    pub fixed_section_ok: bool,
    pub nodes_recovered: u32,
    pub edges_restored: u32,
    pub cross_batch_edges_added: u32,
    pub types_remapped: u32,
    pub complexity_remapped: u32,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmRemapResponse {
    #[serde(default)]
    type_remappings: Vec<LlmKindRemap>,
    #[serde(default)]
    complexity_remappings: Vec<LlmComplexityRemap>,
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmKindRemap {
    from: String,
    to: String,
    /// Optional explicit id-prefix when the kind change should
    /// also rewrite the leading `<old>:` to `<new>:` in the
    /// node id. Defaults to true.
    #[serde(default = "default_true")]
    rewrite_id: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LlmComplexityRemap {
    from: String,
    to: String,
}

/// Run the LLM pass. `scan` is the scan result whose
/// `import_map` we use for cross-batch gap detection. Mutates
/// `graph` in place.
pub async fn assemble_review_llm(
    graph: &mut KnowledgeGraph,
    deterministic_report: &AssembleReport,
    scan: &ScanResult,
    llm: &LlmRequestSpec,
) -> AssembleReviewReport {
    let mut review = AssembleReviewReport {
        fixed_section_ok: true,
        ..Default::default()
    };

    // ----- Step 1: Sanity-check fixed section -----
    if deterministic_report.nodes_renamed > 0
        || deterministic_report.edges_dropped > 0
        || deterministic_report.complexity_normalized > 0
    {
        review.notes.push(format!(
            "deterministic report: {} renamed, {} deduped, {} edges dropped, {} complexity normalized",
            deterministic_report.nodes_renamed,
            deterministic_report.nodes_deduped,
            deterministic_report.edges_dropped,
            deterministic_report.complexity_normalized,
        ));
    }

    // ----- Step 2: Find unknown kinds / complexities and ask LLM for remaps -----
    let mut seen_kinds: HashMap<String, ()> = HashMap::new();
    let unknown_kinds: Vec<String> = graph
        .nodes
        .iter()
        .map(|n| n.kind.clone())
        .filter(|k| !KNOWN_NODE_KINDS.contains(&k.as_str()))
        .filter(|k| seen_kinds.insert(k.clone(), ()).is_none())
        .collect();
    let mut seen_complexities: HashMap<String, ()> = HashMap::new();
    let bad_complexities: Vec<String> = graph
        .nodes
        .iter()
        .map(|n| n.complexity.clone())
        .filter(|c| !matches!(c.as_str(), "simple" | "moderate" | "complex"))
        .filter(|c| seen_complexities.insert(c.clone(), ()).is_none())
        .collect();

    if !unknown_kinds.is_empty() || !bad_complexities.is_empty() {
        let user_payload = json!({
            "unknown_kinds": unknown_kinds,
            "unknown_complexities": bad_complexities,
            "known_node_kinds": KNOWN_NODE_KINDS,
        });
        let user = serde_json::to_string_pretty(&user_payload)
            .unwrap_or_else(|_| "{}".to_string());
        let mut req: LlmRequest = llm.into_request(
            ASSEMBLE_REVIEWER_PROMPT.to_string(),
            user,
        );
        req.temperature = 0.1;
        let resp: Result<LlmResponse, _> = call_llm(req, 1).await;
        match resp {
            Ok(r) => match serde_json::from_str::<LlmRemapResponse>(&strip_fence(&r.content)) {
                Ok(parsed) => {
                    apply_remappings(
                        graph,
                        &parsed,
                        &mut review,
                    );
                }
                Err(e) => {
                    review.notes.push(format!(
                        "assemble-reviewer LLM response could not be parsed: {e}"
                    ));
                }
            },
            Err(e) => {
                review.notes.push(format!("assemble-reviewer LLM call failed: {e:?}"));
            }
        }
    }

    // ----- Step 3: Cross-batch edge gaps -----
    let added = add_missing_import_edges(graph, scan);
    review.cross_batch_edges_added = added;

    review
}

fn apply_remappings(
    graph: &mut KnowledgeGraph,
    parsed: &LlmRemapResponse,
    review: &mut AssembleReviewReport,
) {
    for r in &parsed.type_remappings {
        let from = r.from.to_lowercase();
        let to = r.to.to_lowercase();
        for n in graph.nodes.iter_mut() {
            if n.kind.to_lowercase() == from {
                n.kind = to.clone();
                if r.rewrite_id && n.id.starts_with(&format!("{from}:")) {
                    n.id = format!("{}:{}", to, &n.id[from.len() + 1..]);
                }
                review.types_remapped += 1;
            }
        }
    }
    for r in &parsed.complexity_remappings {
        let from = r.from.to_lowercase();
        let to = r.to.to_lowercase();
        if !matches!(to.as_str(), "simple" | "moderate" | "complex") {
            review.notes.push(format!(
                "ignored complexity remap {} -> {} (target not in canonical set)",
                from, to
            ));
            continue;
        }
        for n in graph.nodes.iter_mut() {
            if n.complexity.to_lowercase() == from {
                n.complexity = to.clone();
                review.complexity_remapped += 1;
            }
        }
    }
    for note in &parsed.notes {
        review.notes.push(note.clone());
    }
}

fn add_missing_import_edges(graph: &mut KnowledgeGraph, scan: &ScanResult) -> u32 {
    let node_ids: std::collections::HashSet<String> =
        graph.nodes.iter().map(|n| n.id.clone()).collect();
    let mut added: u32 = 0;
    for (file_rel, imports) in &scan.import_map {
        let source_id = format!("file:{file_rel}");
        if !node_ids.contains(&source_id) {
            continue;
        }
        for target_rel in imports {
            let target_id = format!("file:{target_rel}");
            if !node_ids.contains(&target_id) {
                continue;
            }
            let already = graph.edges.iter().any(|e| {
                e.source == source_id && e.target == target_id && e.kind == "imports"
            });
            if already {
                continue;
            }
            graph.edges.push(GraphEdge {
                source: source_id.clone(),
                target: target_id,
                kind: "imports".to_string(),
                direction: "forward".to_string(),
                weight: 0.7,
                description: None,
            });
            added += 1;
        }
    }
    added
}

fn strip_fence(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with("```") {
        if let Some(end) = trimmed.rfind("```") {
            let after_open = trimmed.find('\n').map(|i| i + 1).unwrap_or(3);
            return trimmed[after_open..end].trim().to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::ProjectMeta;
    use std::collections::BTreeMap;

    fn n(id: &str, kind: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            kind: kind.to_string(),
            name: id.to_string(),
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

    fn scan_with_imports(map: BTreeMap<String, Vec<String>>) -> ScanResult {
        ScanResult {
            project_root: String::new(),
            files: vec![],
            total_files: 0,
            filtered_by_ignore: 0,
            estimated_complexity: "moderate".to_string(),
            stats: crate::commands::code_wiki_scanner::ScanStats {
                files_scanned: 0,
                by_category: BTreeMap::new(),
                by_language: BTreeMap::new(),
            },
            project_name: String::new(),
            project_description: String::new(),
            frameworks: Vec::new(),
            git_commit_hash: String::new(),
            import_map: map,
        }
    }

    #[test]
    fn apply_remappings_handles_kind_and_complexity() {
        let mut graph = g(
            vec![
                n("func:a.ts:foo", "func"),
                n("file:b.ts", "file"),
                GraphNode {
                    id: "file:c.ts".to_string(),
                    kind: "file".to_string(),
                    name: "c.ts".to_string(),
                    file_path: String::new(),
                    summary: String::new(),
                    tags: vec![],
                    complexity: "trivial".to_string(),
                    location: None,
                    language_notes: None,
                },
            ],
            vec![],
        );
        let parsed = LlmRemapResponse {
            type_remappings: vec![LlmKindRemap {
                from: "func".to_string(),
                to: "function".to_string(),
                rewrite_id: true,
            }],
            complexity_remappings: vec![LlmComplexityRemap {
                from: "trivial".to_string(),
                to: "simple".to_string(),
            }],
            notes: vec!["ok".to_string()],
        };
        let mut review = AssembleReviewReport::default();
        apply_remappings(&mut graph, &parsed, &mut review);
        assert_eq!(review.types_remapped, 1);
        assert_eq!(review.complexity_remapped, 1);
        let f = graph.nodes.iter().find(|n| n.id == "function:a.ts:foo").unwrap();
        assert_eq!(f.kind, "function");
        let c = graph.nodes.iter().find(|n| n.id == "file:c.ts").unwrap();
        assert_eq!(c.complexity, "simple");
        assert!(review.notes.iter().any(|n| n == "ok"));
    }

    #[test]
    fn apply_remappings_rejects_invalid_complexity_target() {
        let mut graph = g(
            vec![GraphNode {
                id: "file:c.ts".to_string(),
                kind: "file".to_string(),
                name: "c.ts".to_string(),
                file_path: String::new(),
                summary: String::new(),
                tags: vec![],
                complexity: "trivial".to_string(),
                location: None,
                language_notes: None,
            }],
            vec![],
        );
        let parsed = LlmRemapResponse {
            type_remappings: vec![],
            complexity_remappings: vec![LlmComplexityRemap {
                from: "trivial".to_string(),
                to: "very easy".to_string(), // not canonical
            }],
            notes: vec![],
        };
        let mut review = AssembleReviewReport::default();
        apply_remappings(&mut graph, &parsed, &mut review);
        assert_eq!(review.complexity_remapped, 0);
        assert!(review
            .notes
            .iter()
            .any(|n| n.contains("ignored complexity remap")));
    }

    #[test]
    fn add_missing_import_edges_inserts_only_missing() {
        let mut graph = g(
            vec![
                n("file:a.ts", "file"),
                n("file:b.ts", "file"),
                n("file:c.ts", "file"),
            ],
            vec![
                // file:a.ts -> file:b.ts already exists
                e("file:a.ts", "file:b.ts", "imports"),
            ],
        );
        let mut map = BTreeMap::new();
        map.insert("a.ts".to_string(), vec!["b.ts".to_string(), "c.ts".to_string()]);
        let added = add_missing_import_edges(&mut graph, &scan_with_imports(map));
        assert_eq!(added, 1, "should only add the missing c.ts edge");
        assert_eq!(
            graph.edges.iter().filter(|e| e.kind == "imports").count(),
            2
        );
    }

    #[test]
    fn add_missing_import_edges_ignores_unknown_files() {
        let mut graph = g(
            vec![n("file:a.ts", "file")],
            vec![],
        );
        let mut map = BTreeMap::new();
        map.insert("a.ts".to_string(), vec!["missing.ts".to_string()]);
        let added = add_missing_import_edges(&mut graph, &scan_with_imports(map));
        assert_eq!(added, 0);
    }

    #[test]
    fn strip_fence_unwraps_code_block() {
        let raw = "```json\n{\"type_remappings\":[]}\n```";
        let body = strip_fence(raw);
        assert!(body.contains("type_remappings"));
    }

    #[test]
    fn strip_fence_passes_through_plain_json() {
        let raw = "{\"notes\":[]}";
        assert_eq!(strip_fence(raw), raw);
    }

    #[test]
    fn known_kinds_includes_codebase_and_knowledge() {
        assert!(KNOWN_NODE_KINDS.contains(&"file"));
        assert!(KNOWN_NODE_KINDS.contains(&"article"));
        assert!(KNOWN_NODE_KINDS.contains(&"domain"));
    }
}