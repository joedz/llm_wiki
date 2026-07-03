// `/understand-explain` — single-node deep-dive explanation.
//
// Loads `knowledge-graph.json`, finds the target node, gathers
// its 1-hop neighborhood (incoming + outgoing edges), reads the
// relevant source-code excerpt, and dispatches an LLM call (or
// falls back to a deterministic template).
//
// Synchronous (no `tokio::spawn`); a typical explanation finishes
// in a few seconds when an LLM is configured.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::commands::code_wiki::{code_wiki_get_graph_inner, graph_path_for};
use crate::commands::code_wiki_pipeline::{GraphEdge, GraphNode, LlmRequestSpec};
use crate::llm_client::{call_llm, LlmRequest, LlmResponse};

const EXPLAINER_PROMPT: &str = include_str!("../prompts/graph_explainer.md");
const MAX_SOURCE_LINES: u32 = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainNeighbor {
    pub node: GraphNode,
    pub edge: GraphEdge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainResult {
    pub node_id: String,
    pub markdown: String,
    pub neighbor_count: u32,
    pub source_lines_read: u32,
    pub used_llm: bool,
    pub duration_ms: u64,
    pub layer: Option<crate::commands::code_wiki_architecture::Layer>,
}

#[tauri::command]
pub async fn code_wiki_explain_node(
    project_path: String,
    repo_name: String,
    node_id: String,
    llm: Option<LlmRequestSpec>,
) -> Result<ExplainResult, String> {
    let started = std::time::Instant::now();
    let project_root = PathBuf::from(&project_path);
    let graph = code_wiki_get_graph_inner(&project_root, &repo_name)?
        .ok_or_else(|| format!("knowledge graph missing for {repo_name}; run Analyze first"))?;
    let target = graph
        .nodes
        .iter()
        .find(|n| n.id == node_id)
        .cloned()
        .ok_or_else(|| format!("node {node_id} not found in {repo_name}"))?;

    // 1-hop neighbors: split into incoming + outgoing
    let node_index: std::collections::HashMap<&str, &GraphNode> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut incoming: Vec<ExplainNeighbor> = Vec::new();
    let mut outgoing: Vec<ExplainNeighbor> = Vec::new();
    for e in &graph.edges {
        if e.source == node_id {
            if let Some(neighbor) = node_index.get(e.target.as_str()) {
                outgoing.push(ExplainNeighbor {
                    node: (*neighbor).clone(),
                    edge: e.clone(),
                });
            }
        }
        if e.target == node_id {
            if let Some(neighbor) = node_index.get(e.source.as_str()) {
                incoming.push(ExplainNeighbor {
                    node: (*neighbor).clone(),
                    edge: e.clone(),
                });
            }
        }
    }
    let neighbor_count = (incoming.len() + outgoing.len()) as u32;

    // Find layer (if any)
    let layer = graph
        .layers
        .iter()
        .find(|l| l.node_ids.iter().any(|id| id == &node_id))
        .cloned();

    // Source excerpt (if file_path + location available)
    let (source_excerpt, source_lines_read) =
        read_source_excerpt(&project_root, &repo_name, &target);

    let used_llm;
    let markdown = if let Some(spec) = llm {
        let payload = json!({
            "node": target,
            "layer": layer,
            "incoming": incoming,
            "outgoing": outgoing,
            "source_excerpt": source_excerpt,
        });
        let user = serde_json::to_string_pretty(&payload)
            .map_err(|e| format!("serialize payload: {e}"))?;
        let mut req: LlmRequest = spec.into_request(EXPLAINER_PROMPT.to_string(), user);
        req.temperature = 0.2;
        match call_llm(req, 1).await {
            Ok(resp) => {
                used_llm = true;
                resp.content
            }
            Err(e) => {
                used_llm = false;
                format!(
                    "_LLM explainer call failed: {e:?}. Falling back to template._\n\n{}",
                    template_explanation(&target, &incoming, &outgoing, &layer, &source_excerpt)
                )
            }
        }
    } else {
        used_llm = false;
        template_explanation(&target, &incoming, &outgoing, &layer, &source_excerpt)
    };

    Ok(ExplainResult {
        node_id: target.id,
        markdown,
        neighbor_count,
        source_lines_read,
        used_llm,
        duration_ms: started.elapsed().as_millis() as u64,
        layer,
    })
}

fn read_source_excerpt(
    project_root: &Path,
    repo_name: &str,
    node: &GraphNode,
) -> (String, u32) {
    if node.file_path.is_empty() {
        return (String::new(), 0);
    }
    let abs = project_root
        .join("raw")
        .join("code")
        .join(repo_name)
        .join(&node.file_path);
    let Ok(content) = fs::read_to_string(&abs) else {
        return (String::new(), 0);
    };
    let lines: Vec<&str> = content.lines().collect();
    let (start, end) = match &node.location {
        Some(loc) => {
            let s = (loc.start_line as usize).saturating_sub(1);
            let e = (loc.end_line as usize).min(lines.len());
            (s, e)
        }
        None => (0usize, MAX_SOURCE_LINES as usize),
    };
    let end = end.min(lines.len());
    if start >= end || start >= lines.len() {
        return (String::new(), 0);
    }
    let mut excerpt = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        excerpt.push_str(&format!("{:>4}  {}\n", start + i + 1, line));
    }
    let read = (end - start) as u32;
    (excerpt, read)
}

fn template_explanation(
    node: &GraphNode,
    incoming: &[ExplainNeighbor],
    outgoing: &[ExplainNeighbor],
    layer: &Option<crate::commands::code_wiki_architecture::Layer>,
    source_excerpt: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("## `{}`\n\n", node.name));
    out.push_str(&format!("**Type:** `{}`\n\n", node.kind));
    if !node.file_path.is_empty() {
        out.push_str(&format!("**File:** `{}`\n\n", node.file_path));
    }
    if !node.summary.is_empty() {
        out.push_str(&format!("**Summary:** {}\n\n", node.summary));
    }
    if !node.tags.is_empty() {
        out.push_str(&format!("**Tags:** {}\n\n", node.tags.join(", ")));
    }
    if let Some(l) = layer {
        let descr = if l.description.is_empty() {
            "(no description)".to_string()
        } else {
            l.description.clone()
        };
        out.push_str(&format!("**Layer:** `{}` — {}\n\n", l.name, descr));
    }
    if !outgoing.is_empty() {
        out.push_str("### Calls / depends on\n\n");
        for n in outgoing {
            out.push_str(&format!(
                "- `[{}:{}]` `{}` — {}\n",
                n.node.kind,
                n.node.id,
                n.node.name,
                n.node.summary.chars().take(80).collect::<String>()
            ));
        }
        out.push('\n');
    }
    if !incoming.is_empty() {
        out.push_str("### Called by / depended on by\n\n");
        for n in incoming {
            out.push_str(&format!(
                "- `[{}:{}]` `{}` — {}\n",
                n.node.kind,
                n.node.id,
                n.node.name,
                n.node.summary.chars().take(80).collect::<String>()
            ));
        }
        out.push('\n');
    }
    if !source_excerpt.is_empty() {
        out.push_str("### Source excerpt\n\n```\n");
        out.push_str(source_excerpt);
        out.push_str("```\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::{KnowledgeGraph, ProjectMeta};

    fn write_graph(project: &Path, repo: &str, graph: &KnowledgeGraph) -> PathBuf {
        let path = graph_path_for(project, repo);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let bytes = serde_json::to_vec_pretty(graph).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn node(id: &str, kind: &str, name: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            file_path: String::new(),
            summary: String::new(),
            tags: vec![],
            complexity: "moderate".to_string(),
            location: None,
            language_notes: None,
        }
    }

    fn edge(source: &str, target: &str, kind: &str) -> GraphEdge {
        GraphEdge {
            source: source.to_string(),
            target: target.to_string(),
            kind: kind.to_string(),
            direction: "forward".to_string(),
            weight: 1.0,
            description: None,
        }
    }

    #[tokio::test]
    async fn explain_returns_node_not_found_error() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let repo = "demo";
        let graph = KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: repo.to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![node("file:src/a.ts", "file", "a.ts")],
            edges: vec![],
            layers: vec![],
            tour: vec![],
        };
        write_graph(project, repo, &graph);
        let res = code_wiki_explain_node(
            project.to_string_lossy().to_string(),
            repo.to_string(),
            "file:does-not-exist".to_string(),
            None,
        )
        .await;
        assert!(res.is_err());
        let msg = res.unwrap_err();
        assert!(msg.contains("not found"), "unexpected error: {msg}");
    }

    #[tokio::test]
    async fn explain_falls_back_to_template_without_llm() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let repo = "demo";
        let graph = KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: repo.to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![
                GraphNode {
                    id: "function:src/auth.ts:verify".to_string(),
                    kind: "function".to_string(),
                    name: "verify".to_string(),
                    file_path: "src/auth.ts".to_string(),
                    summary: "JWT verify helper".to_string(),
                    tags: vec!["auth".to_string()],
                    complexity: "moderate".to_string(),
                    location: None,
                    language_notes: None,
                },
                node("function:src/login.ts:handler", "function", "handler"),
            ],
            edges: vec![edge("function:src/login.ts:handler", "function:src/auth.ts:verify", "calls")],
            layers: vec![],
            tour: vec![],
        };
        write_graph(project, repo, &graph);
        let res = code_wiki_explain_node(
            project.to_string_lossy().to_string(),
            repo.to_string(),
            "function:src/auth.ts:verify".to_string(),
            None,
        )
        .await
        .expect("should succeed");
        assert!(!res.used_llm);
        assert!(res.markdown.contains("verify"));
        assert!(res.markdown.contains("JWT verify helper"));
        assert!(res.neighbor_count >= 1);
    }

    #[tokio::test]
    async fn explain_returns_missing_graph_error() {
        let dir = tempfile::tempdir().unwrap();
        let res = code_wiki_explain_node(
            dir.path().to_string_lossy().to_string(),
            "ghost".to_string(),
            "file:any".to_string(),
            None,
        )
        .await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("missing"));
    }

    #[tokio::test]
    async fn explain_includes_source_excerpt_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let raw_code = project.join("raw").join("code");
        let src = raw_code.join("demo").join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.ts"), "function hello() { return 42; }\n").unwrap();
        let repo = "demo";
        let graph = KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: repo.to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![GraphNode {
                id: "function:src/a.ts:hello".to_string(),
                kind: "function".to_string(),
                name: "hello".to_string(),
                file_path: "src/a.ts".to_string(),
                summary: "Says hi".to_string(),
                tags: vec![],
                complexity: "simple".to_string(),
                location: None,
                language_notes: None,
            }],
            edges: vec![],
            layers: vec![],
            tour: vec![],
        };
        write_graph(project, repo, &graph);
        let res = code_wiki_explain_node(
            project.to_string_lossy().to_string(),
            repo.to_string(),
            "function:src/a.ts:hello".to_string(),
            None,
        )
        .await
        .expect("ok");
        assert!(
            res.source_lines_read >= 1,
            "should have read source, got res={:?}",
            res
        );
        assert!(res.markdown.contains("hello"));
    }

    #[test]
    fn template_includes_layer_info() {
        let n = GraphNode {
            id: "function:src/a.ts:f".to_string(),
            kind: "function".to_string(),
            name: "f".to_string(),
            file_path: "src/a.ts".to_string(),
            summary: "S".to_string(),
            tags: vec![],
            complexity: "moderate".to_string(),
            location: None,
            language_notes: None,
        };
        let layer = Some(crate::commands::code_wiki_architecture::Layer {
            id: "layer:x".to_string(),
            name: "Auth".to_string(),
            description: "JWT handling".to_string(),
            node_ids: vec![n.id.clone()],
        });
        let s = template_explanation(&n, &[], &[], &layer, "");
        assert!(s.contains("Auth"));
        assert!(s.contains("JWT handling"));
    }

    #[test]
    fn empty_template_still_renders() {
        let n = GraphNode {
            id: "concept:x".to_string(),
            kind: "concept".to_string(),
            name: "x".to_string(),
            file_path: String::new(),
            summary: String::new(),
            tags: vec![],
            complexity: "moderate".to_string(),
            location: None,
            language_notes: None,
        };
        let s = template_explanation(&n, &[], &[], &None, "");
        assert!(s.contains("concept"));
        assert!(s.contains("x"));
    }
}