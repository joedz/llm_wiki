// LLM-driven graph reviewer (UA's `--review` mode).
//
// UA's `graph-reviewer` agent is a 2-phase validator:
//   Phase 1 — write + execute a deterministic script (Node.js or Python)
//             that reads the graph JSON, runs schema / referential /
//             completeness / uniqueness / quality checks, and
//             writes the result back as JSON.
//   Phase 2 — the LLM reviews the script's findings and renders
//             an `approved` decision plus a short narrative.
//
// In the Rust port, we replicate Phase 1 directly inside
// `code_wiki_reviewer.rs::review_graph` (already exists), so the
// LLM only needs to do Phase 2. The LLM is given the
// deterministic report and a compact graph summary; it produces
// `{approved, issues, warnings, stats, narrative}`. The
// `narrative` field is a human-readable paragraph we surface in
// `meta.json` so the dashboard can show it.
//
// Cost: 1 LLM call per pipeline run when `--review` is set.
// Failure: LLM errors are recorded as a warning, never fail
// the pipeline (UA's "always save partial" principle).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::commands::code_wiki_pipeline::KnowledgeGraph;
use crate::commands::code_wiki_reviewer::ReviewReport;
use crate::llm_client::{call_llm, LlmRequest, LlmResponse};

const GRAPH_REVIEWER_PROMPT: &str = include_str!("../prompts/graph_reviewer.md");

/// JSON shape expected back from the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmReviewVerdict {
    pub approved: bool,
    #[serde(default)]
    pub issues: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub narrative: String,
    #[serde(default)]
    pub stats: ReviewerStats,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ReviewerStats {
    #[serde(default, rename = "totalNodes")]
    pub total_nodes: u32,
    #[serde(default, rename = "totalEdges")]
    pub total_edges: u32,
    #[serde(default, rename = "totalLayers")]
    pub total_layers: u32,
    #[serde(default, rename = "tourSteps")]
    pub tour_steps: u32,
    #[serde(default, rename = "nodeTypes")]
    pub node_types: BTreeMap<String, u32>,
    #[serde(default, rename = "edgeTypes")]
    pub edge_types: BTreeMap<String, u32>,
}

/// Build the user prompt sent to the LLM. Bundles the
/// deterministic review output + a small graph summary so the
/// LLM doesn't have to re-read the entire graph.
pub fn build_user_prompt(
    deterministic: &ReviewReport,
    graph: &KnowledgeGraph,
) -> String {
    let mut node_types: BTreeMap<String, u32> = BTreeMap::new();
    for n in &graph.nodes {
        *node_types.entry(n.kind.clone()).or_insert(0) += 1;
    }
    let mut edge_types: BTreeMap<String, u32> = BTreeMap::new();
    for e in &graph.edges {
        *edge_types.entry(e.kind.clone()).or_insert(0) += 1;
    }

    let stats = json!({
        "totalNodes": graph.nodes.len(),
        "totalEdges": graph.edges.len(),
        "totalLayers": graph.layers.len(),
        "tourSteps": graph.tour.len(),
        "nodeTypes": node_types,
        "edgeTypes": edge_types,
    });

    let det_issues: Vec<String> = deterministic
        .issues
        .iter()
        .map(|i| format!("[{}|{}] {}", i.level, i.category, i.message))
        .collect();

    let user = json!({
        "deterministicReview": {
            "issueCount": deterministic.issues.len(),
            "warningCount": deterministic.issues.iter().filter(|i| i.level == "warning").count(),
            "orphanNodes": deterministic.stats.orphan_nodes,
            "issues": det_issues,
            "stats": &deterministic.stats,
        },
        "graphStats": stats,
    });
    serde_json::to_string_pretty(&user).unwrap_or_else(|_| "{}".to_string())
}

/// Call the LLM with the embedded prompt. Markdown-fence
/// stripping is automatic. Returns the parsed verdict or an
/// error.
pub async fn call_graph_reviewer(
    llm_request: &super::code_wiki_pipeline::LlmRequestSpec,
    deterministic: &ReviewReport,
    graph: &KnowledgeGraph,
) -> Result<LlmReviewVerdict, String> {
    let system = GRAPH_REVIEWER_PROMPT.to_string();
    let user = build_user_prompt(deterministic, graph);
    let mut req: LlmRequest = llm_request.into_request(system, user);
    // We want consistent, structured JSON from the LLM — low
    // temperature keeps it deterministic enough for our parser
    // to succeed.
    req.temperature = 0.1;
    let resp: LlmResponse = call_llm(req, 1)
        .await
        .map_err(|e| format!("LLM call failed: {e:?}"))?;
    parse_verdict_response(&resp.content)
}

/// Parse the LLM's JSON response. Strips markdown code fences
/// before JSON extraction.
pub fn parse_verdict_response(content: &str) -> Result<LlmReviewVerdict, String> {
    let trimmed = content.trim();
    let body = if trimmed.starts_with("```") {
        if let Some(end) = trimmed.rfind("```") {
            let after_open = trimmed.find('\n').map(|i| i + 1).unwrap_or(3);
            trimmed[after_open..end].trim()
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    let parsed: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| format!("response not valid JSON: {e}\n---\n{body}\n---"))?;
    let obj = parsed
        .as_object()
        .ok_or_else(|| "response is not a JSON object".to_string())?;
    let verdict: LlmReviewVerdict = serde_json::from_value(parsed.clone())
        .map_err(|e| format!("verdict shape invalid: {e}"))?;
    let _ = obj;
    Ok(verdict)
}

/// Build the JSON snippet embedded into `meta.json` after a
/// successful LLM review.
pub fn narrative_for_meta(verdict: &LlmReviewVerdict) -> serde_json::Value {
    json!({
        "approved": verdict.approved,
        "issues": verdict.issues,
        "warnings": verdict.warnings,
        "narrative": verdict.narrative,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::GraphNode;
    use crate::commands::code_wiki_reviewer::{GraphStats, ReviewIssue, ReviewReport};

    fn sample_graph() -> KnowledgeGraph {
        KnowledgeGraph {
            version: "1.0.0".into(),
            kind: "codebase".into(),
            project: crate::commands::code_wiki_pipeline::ProjectMeta {
                name: "demo".into(),
                languages: vec!["rust".into()],
                frameworks: vec![],
                description: "demo".into(),
                analyzed_at: "2026-07-02T00:00:00.000Z".into(),
                git_commit_hash: "deadbeef".into(),
            },
            nodes: vec![
                GraphNode {
                    id: "file:src/main.rs".into(),
                    kind: "file".into(),
                    name: "main.rs".into(),
                    file_path: "src/main.rs".into(),
                    summary: "demo".into(),
                    tags: vec![],
                    complexity: "moderate".into(),
                    location: None,
                    language_notes: None,
                },
                GraphNode {
                    id: "function:src/main.rs:hello".into(),
                    kind: "function".into(),
                    name: "hello".into(),
                    file_path: "src/main.rs".into(),
                    summary: "says hi".into(),
                    tags: vec![],
                    complexity: "simple".into(),
                    location: None,
                    language_notes: None,
                },
            ],
            edges: vec![],
            layers: vec![],
            tour: vec![],
        }
    }

    fn sample_report() -> ReviewReport {
        ReviewReport {
            issues: vec![ReviewIssue {
                level: "warning".into(),
                category: "orphan".into(),
                message: "1 orphan node".into(),
                path: Some("function:src/main.rs:hello".into()),
            }],
            stats: GraphStats {
                total_nodes: 2,
                total_edges: 0,
                total_layers: 0,
                tour_steps: 0,
                by_node_type: Default::default(),
                by_edge_type: Default::default(),
                orphan_nodes: 1,
                languages: Default::default(),
            },
            missing_edges: vec![],
        }
    }

    #[test]
    fn build_user_prompt_contains_deterministic_summary() {
        let user = build_user_prompt(&sample_report(), &sample_graph());
        assert!(user.contains("deterministicReview"));
        assert!(user.contains("graphStats"));
        assert!(user.contains("\"totalNodes\": 2"));
    }

    #[test]
    fn parse_verdict_strips_code_fence_and_reads_fields() {
        let body = "```json\n{\"approved\":true,\"issues\":[],\"warnings\":[\"x\"],\"narrative\":\"Looks good.\",\"stats\":{\"totalNodes\":2,\"totalEdges\":0,\"totalLayers\":0,\"tourSteps\":0,\"nodeTypes\":{},\"edgeTypes\":{}}}\n```";
        let v = parse_verdict_response(body).expect("parse");
        assert!(v.approved);
        assert_eq!(v.warnings.len(), 1);
        assert_eq!(v.narrative, "Looks good.");
    }

    #[test]
    fn parse_verdict_rejects_invalid_json() {
        let body = "not even close to JSON";
        let r = parse_verdict_response(body);
        assert!(r.is_err());
    }
}
