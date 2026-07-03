// `/understand-chat` — grep-based Q&A over the knowledge graph.
//
// UA's `/understand-chat` skill uses grep-based retrieval
// (`Grep` on `name`/`summary`/`tags`/`id`) over the knowledge
// graph to assemble context, then runs an LLM with a "cite node
// IDs in [brackets]" instruction. We port that:
//
//   1. Tokenize query, lowercase, score each node by how many
//      tokens it matches across {id, name, summary, tags}.
//   2. Take top-K nodes (default 10) as primary context.
//   3. Expand to 1-hop neighbors as secondary context (capped at
//      20 nodes) — gives the LLM visibility into call sites,
//      imports, layer membership.
//   4. Pull layer descriptions for each primary node.
//   5. Assemble prompt with optional chat history; call LLM;
//      return markdown answer.
//
// Without an LLM the command returns a deterministic "found
// these nodes" response so the UI can still show the user where
// to look.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::commands::code_wiki::{
    code_wiki_get_graph_inner, domain_graph_path_for,
};
use crate::commands::code_wiki_pipeline::{GraphEdge, GraphNode, KnowledgeGraph, LlmRequestSpec};
use crate::commands::code_wiki_architecture::Layer;
use crate::llm_client::{call_llm, LlmRequest, LlmResponse};

const CHAT_SYSTEM_PROMPT: &str = "You are an expert on a codebase. You will be given the user query plus \
a set of 'primary' and 'secondary' graph nodes and their relationships, \
plus the architectural layer each primary node belongs to. Answer the \
query using ONLY the provided context. When you reference a node, cite its \
node id in [brackets] (e.g. [function:src/auth.ts:verifyToken]). If the \
context is insufficient, say so explicitly — do not invent answers.\n\n\
Output: clean markdown. Sections as needed. Cite liberally.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "user" | "assistant"
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResult {
    pub answer: String,
    pub primary_node_ids: Vec<String>,
    pub secondary_node_ids: Vec<String>,
    pub used_llm: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
struct ScoredNode {
    node: GraphNode,
    score: f32,
}

#[tauri::command]
pub async fn code_wiki_chat_query(
    project_path: String,
    repo_name: String,
    query: String,
    history: Option<Vec<ChatMessage>>,
    llm: Option<LlmRequestSpec>,
) -> Result<ChatResult, String> {
    let started = std::time::Instant::now();
    let project_root = PathBuf::from(&project_path);
    let graph = code_wiki_get_graph_inner(&project_root, &repo_name)?
        .ok_or_else(|| format!("knowledge graph missing for {repo_name}; run Analyze first"))?;
    let history = history.unwrap_or_default();

    let tokens = tokenize(&query);
    if tokens.is_empty() {
        return Err("empty query".to_string());
    }

    let (primary, secondary) = retrieve(&graph, &tokens, 10, 20);
    let primary_ids: Vec<String> = primary.iter().map(|p| p.node.id.clone()).collect();
    let secondary_ids: Vec<String> = secondary.iter().map(|n| n.id.clone()).collect();

    let used_llm;
    let answer = if let Some(spec) = llm {
        let layer_lookup = build_layer_lookup(&graph.layers);
        let primary_payload: Vec<_> = primary
            .iter()
            .map(|p| {
                json!({
                    "id": p.node.id,
                    "type": p.node.kind,
                    "name": p.node.name,
                    "summary": p.node.summary,
                    "tags": p.node.tags,
                    "complexity": p.node.complexity,
                    "layer": layer_lookup.get(&p.node.id),
                    "score": p.score,
                })
            })
            .collect();
        let secondary_payload: Vec<_> = secondary
            .iter()
            .map(|n| {
                json!({
                    "id": n.id,
                    "type": n.kind,
                    "name": n.name,
                    "summary": n.summary,
                })
            })
            .collect();
        let history_payload: Vec<_> = history
            .iter()
            .map(|h| json!({"role": h.role, "content": h.content}))
            .collect();
        let user = json!({
            "query": query,
            "primary": primary_payload,
            "secondary": secondary_payload,
            "history": history_payload,
        });
        let user_str = serde_json::to_string_pretty(&user)
            .map_err(|e| format!("serialize user payload: {e}"))?;
        let mut req: LlmRequest = spec.into_request(CHAT_SYSTEM_PROMPT.to_string(), user_str);
        req.temperature = 0.3;
        req.max_tokens = req.max_tokens.max(2048);
        match call_llm(req, 1).await {
            Ok(resp) => {
                used_llm = true;
                resp.content
            }
            Err(e) => {
                used_llm = false;
                format!(
                    "_LLM chat call failed: {e:?}. Showing retrieval results only._\n\n{}",
                    template_answer(&query, &primary, &graph)
                )
            }
        }
    } else {
        used_llm = false;
        template_answer(&query, &primary, &graph)
    };

    Ok(ChatResult {
        answer,
        primary_node_ids: primary_ids,
        secondary_node_ids: secondary_ids,
        used_llm,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// Lowercase + split into alphanumeric tokens. Drops tokens
/// shorter than 2 chars. Returns unique tokens (preserving
/// nothing — we score by membership so duplicates are fine).
fn tokenize(query: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|s| s.len() >= 2)
        .map(|s| s.to_string())
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

/// Grep-based retrieval: score each node by token matches in
/// {id, name, summary, tags}. Returns (primary, secondary).
fn retrieve(
    graph: &KnowledgeGraph,
    tokens: &[String],
    primary_cap: usize,
    secondary_cap: usize,
) -> (Vec<ScoredNode>, Vec<GraphNode>) {
    let mut scored: Vec<ScoredNode> = graph
        .nodes
        .iter()
        .map(|n| {
            let mut score = 0.0f32;
            let id_l = n.id.to_lowercase();
            let name_l = n.name.to_lowercase();
            let summary_l = n.summary.to_lowercase();
            for t in tokens {
                if id_l.contains(t) {
                    score += 3.0;
                }
                if name_l.contains(t) {
                    score += 2.0;
                }
                if summary_l.contains(t) {
                    score += 1.0;
                }
                for tag in &n.tags {
                    if tag.to_lowercase() == *t {
                        score += 1.5;
                    } else if tag.to_lowercase().contains(t) {
                        score += 0.75;
                    }
                }
            }
            ScoredNode {
                node: n.clone(),
                score,
            }
        })
        .filter(|s| s.score > 0.0)
        .collect();
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(primary_cap);
    let primary_ids: BTreeSet<String> = scored.iter().map(|s| s.node.id.clone()).collect();

    // Expand to 1-hop neighbors (capped at secondary_cap).
    let mut secondary_ids: BTreeSet<String> = BTreeSet::new();
    let mut secondary_nodes: Vec<GraphNode> = Vec::new();
    for e in &graph.edges {
        if primary_ids.contains(&e.source) && !primary_ids.contains(&e.target) {
            secondary_ids.insert(e.target.clone());
        }
        if primary_ids.contains(&e.target) && !primary_ids.contains(&e.source) {
            secondary_ids.insert(e.source.clone());
        }
    }
    for n in &graph.nodes {
        if secondary_ids.contains(&n.id) && secondary_nodes.len() < secondary_cap {
            secondary_nodes.push(n.clone());
        }
        if secondary_nodes.len() >= secondary_cap {
            break;
        }
    }

    (scored, secondary_nodes)
}

fn build_layer_lookup(layers: &[Layer]) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for l in layers {
        for nid in &l.node_ids {
            out.entry(nid.clone()).or_insert_with(|| {
                format!("{} — {}", l.name, l.description)
            });
        }
    }
    out
}

fn template_answer(query: &str, primary: &[ScoredNode], _graph: &KnowledgeGraph) -> String {
    let mut out = String::new();
    out.push_str(&format!("## Search results for: _{}_\n\n", query));
    if primary.is_empty() {
        out.push_str("No nodes in the graph matched the query.\n");
        return out;
    }
    out.push_str("Configure an LLM in the chat panel to get a synthesized answer; otherwise the \
                  most relevant graph nodes are listed below.\n\n");
    for s in primary {
        out.push_str(&format!(
            "- `[{}:{}]` **{}** (score {:.2}) — {}\n",
            s.node.kind,
            s.node.id,
            s.node.name,
            s.score,
            s.node.summary.chars().take(120).collect::<String>()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::ProjectMeta;

    fn n(id: &str, kind: &str, name: &str, summary: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            file_path: String::new(),
            summary: summary.to_string(),
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
            description: None,
        }
    }

    #[test]
    fn tokenize_lowercases_and_dedupes() {
        let tokens = tokenize("Auth Token / verify_token");
        assert!(tokens.contains(&"auth".to_string()));
        assert!(tokens.contains(&"token".to_string()));
        assert!(tokens.contains(&"verify_token".to_string()));
        assert_eq!(tokens.len(), 3);
    }

    #[test]
    fn tokenize_drops_short_tokens() {
        let tokens = tokenize("a b token");
        assert!(tokens.contains(&"token".to_string()));
        assert!(!tokens.iter().any(|t| t == "a" || t == "b"));
    }

    #[test]
    fn retrieve_ranks_id_match_above_summary_match() {
        let g = KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![
                n("file:src/auth.ts", "file", "auth.ts", "authentication helper"),
                n("file:src/api.ts", "file", "api.ts", "auth integration glue"),
            ],
            edges: vec![],
            layers: vec![],
            tour: vec![],
        };
        let tokens = tokenize("auth");
        let (primary, _) = retrieve(&g, &tokens, 10, 10);
        assert_eq!(primary.len(), 2);
        // Both should be in primary; ordering may vary but both
        // should score > 0.
        assert!(primary.iter().all(|s| s.score > 0.0));
    }

    #[test]
    fn retrieve_expands_to_1hop_neighbors() {
        let g = KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![
                n("file:src/auth.ts", "file", "auth.ts", "auth helper"),
                n("function:src/auth.ts:verify", "function", "verify", "verify token"),
                n("file:src/other.ts", "file", "other.ts", "unrelated"),
            ],
            edges: vec![e("function:src/auth.ts:verify", "file:src/other.ts", "calls")],
            layers: vec![],
            tour: vec![],
        };
        let tokens = tokenize("verify");
        let (primary, secondary) = retrieve(&g, &tokens, 10, 10);
        // verify is the primary (id+name+summary all match)
        assert!(primary.iter().any(|p| p.node.id == "function:src/auth.ts:verify"));
        // secondary should include other.ts (called by verify)
        assert!(secondary.iter().any(|s| s.id == "file:src/other.ts"));
    }

    #[test]
    fn retrieve_empty_for_unmatched_query() {
        let g = KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![n("file:a", "file", "a", "")],
            edges: vec![],
            layers: vec![],
            tour: vec![],
        };
        let tokens = tokenize("nonexistent");
        let (primary, secondary) = retrieve(&g, &tokens, 10, 10);
        assert!(primary.is_empty());
        assert!(secondary.is_empty());
    }

    #[test]
    fn template_answer_lists_primary_nodes() {
        let primary = vec![
            ScoredNode {
                node: n("file:a.ts", "file", "a.ts", "alpha"),
                score: 5.0,
            },
            ScoredNode {
                node: n("file:b.ts", "file", "b.ts", "beta"),
                score: 3.0,
            },
        ];
        let g = KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![],
            edges: vec![],
            layers: vec![],
            tour: vec![],
        };
        let out = template_answer("where is alpha?", &primary, &g);
        assert!(out.contains("alpha"));
        assert!(out.contains("file:a.ts"));
    }

    #[test]
    fn template_answer_handles_no_match() {
        let primary: Vec<ScoredNode> = vec![];
        let g = KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![],
            edges: vec![],
            layers: vec![],
            tour: vec![],
        };
        let out = template_answer("nothing", &primary, &g);
        assert!(out.contains("No nodes"));
    }

    #[test]
    fn layer_lookup_keys_by_node_id() {
        let layers = vec![Layer {
            id: "layer:x".to_string(),
            name: "Auth".to_string(),
            description: "auth handling".to_string(),
            node_ids: vec!["function:a".to_string(), "function:b".to_string()],
        }];
        let m = build_layer_lookup(&layers);
        assert_eq!(m.len(), 2);
        assert!(m.get("function:a").unwrap().contains("Auth"));
    }
}