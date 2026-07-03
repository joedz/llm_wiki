// `/understand-onboard` — 6-section onboarding markdown guide.
//
// Loads `knowledge-graph.json` and assembles the 6 sections
// (Project Overview / Architecture Layers / Key Concepts /
// Guided Tour / File Map / Complexity Hotspots). With an LLM
// configured, dispatches a single call to format the sections
// as polished markdown; without an LLM, falls back to a
// deterministic template.
//
// Persists to `wiki/code_wiki/<repo>/onboarding.md` so the
// document can be committed and shared with the team. Returns
// the markdown text and the path it was written to.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::commands::code_wiki::{code_wiki_get_graph_inner, meta_path_for};
use crate::commands::code_wiki_pipeline::{GraphNode, KnowledgeGraph, LlmRequestSpec};
use crate::commands::code_wiki_tour::TourStep;
use crate::llm_client::{call_llm, LlmRequest, LlmResponse};

const ONBOARD_PROMPT: &str = include_str!("../prompts/onboard_writer.md");
const ONBOARD_FILE: &str = "onboarding.md";
const TOP_CONCEPTS_COUNT: usize = 10;
const HOTSPOT_COUNT: usize = 20;
const HOTSPOT_MIN_TAGS: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardResult {
    pub markdown: String,
    pub path: String,
    pub used_llm: bool,
    pub duration_ms: u64,
}

#[tauri::command]
pub async fn code_wiki_generate_onboarding(
    project_path: String,
    repo_name: String,
    llm: Option<LlmRequestSpec>,
) -> Result<OnboardResult, String> {
    let started = std::time::Instant::now();
    let project_root = PathBuf::from(&project_path);
    let graph = code_wiki_get_graph_inner(&project_root, &repo_name)?
        .ok_or_else(|| format!("knowledge graph missing for {repo_name}; run Analyze first"))?;
    let sections = assemble_sections(&graph);
    let (used_llm, markdown) = if let Some(spec) = llm {
        let user_payload = serde_json::to_string_pretty(&sections)
            .map_err(|e| format!("serialize sections: {e}"))?;
        let mut req: LlmRequest = spec.into_request(ONBOARD_PROMPT.to_string(), user_payload);
        req.temperature = 0.3;
        req.max_tokens = req.max_tokens.max(4096);
        match call_llm(req, 1).await {
            Ok(resp) => (true, resp.content),
            Err(e) => {
                let warn = format!("_LLM onboard writer failed: {e:?}. Falling back to template._\n\n");
                (false, format!("{warn}{}", render_template(&sections)))
            }
        }
    } else {
        (false, render_template(&sections))
    };

    let out_path = meta_path_for(&project_root, &repo_name)
        .parent()
        .map(|p| p.join(ONBOARD_FILE))
        .unwrap_or_else(|| project_root.join(ONBOARD_FILE));
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    fs::write(&out_path, &markdown).map_err(|e| format!("write onboarding.md: {e}"))?;

    Ok(OnboardResult {
        markdown,
        path: out_path.to_string_lossy().to_string(),
        used_llm,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

#[derive(Debug, Serialize)]
struct OnboardSections {
    project: serde_json::Value,
    layers: Vec<serde_json::Value>,
    key_concepts: Vec<serde_json::Value>,
    tour: Vec<serde_json::Value>,
    file_map: Vec<serde_json::Value>,
    hotspots: Vec<serde_json::Value>,
}

fn assemble_sections(graph: &KnowledgeGraph) -> OnboardSections {
    // 1. Project metadata
    let project = json!({
        "name": graph.project.name,
        "languages": graph.project.languages,
        "frameworks": graph.project.frameworks,
        "description": graph.project.description,
        "analyzedAt": graph.project.analyzed_at,
        "gitCommitHash": graph.project.git_commit_hash,
    });

    // 2. Layers with node counts
    let layers: Vec<serde_json::Value> = graph
        .layers
        .iter()
        .map(|l| {
            json!({
                "id": l.id,
                "name": l.name,
                "description": l.description,
                "nodeCount": l.node_ids.len(),
            })
        })
        .collect();

    // 3. Key concepts: file nodes with tags.length >= 3, top 10
    let mut concept_candidates: Vec<&GraphNode> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == "file" && n.tags.len() >= HOTSPOT_MIN_TAGS)
        .collect();
    concept_candidates.sort_by(|a, b| b.tags.len().cmp(&a.tags.len()));
    concept_candidates.truncate(TOP_CONCEPTS_COUNT);
    let key_concepts: Vec<serde_json::Value> = concept_candidates
        .into_iter()
        .map(|n| {
            json!({
                "id": n.id,
                "name": n.name,
                "filePath": n.file_path,
                "tags": n.tags,
                "summary": n.summary,
            })
        })
        .collect();

    // 4. Tour
    let tour: Vec<serde_json::Value> = graph
        .tour
        .iter()
        .map(|t| {
            json!({
                "order": t.order,
                "title": t.title,
                "description": t.description,
                "nodeIds": t.node_ids,
            })
        })
        .collect();

    // 5. File map: file nodes grouped by layer
    let layer_for_node = build_layer_lookup(&graph.layers);
    let mut by_layer: BTreeMap<String, Vec<&GraphNode>> = BTreeMap::new();
    for n in graph.nodes.iter().filter(|n| n.kind == "file") {
        let layer = layer_for_node
            .get(&n.id)
            .cloned()
            .unwrap_or_else(|| "Unassigned".to_string());
        by_layer.entry(layer).or_default().push(n);
    }
    let file_map: Vec<serde_json::Value> = by_layer
        .into_iter()
        .map(|(layer, files)| {
            json!({
                "layer": layer,
                "files": files.iter().map(|n| json!({
                    "id": n.id,
                    "filePath": n.file_path,
                    "name": n.name,
                    "summary": n.summary,
                    "complexity": n.complexity,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    // 6. Complexity hotspots: top 20 by degree (sum of incident edges)
    let degrees = compute_degrees(graph);
    let mut ranked: Vec<(&GraphNode, u32)> = graph
        .nodes
        .iter()
        .filter(|n| n.complexity == "complex")
        .map(|n| (n, *degrees.get(&n.id).unwrap_or(&0)))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    ranked.truncate(HOTSPOT_COUNT);
    let hotspots: Vec<serde_json::Value> = ranked
        .into_iter()
        .map(|(n, deg)| {
            json!({
                "id": n.id,
                "name": n.name,
                "filePath": n.file_path,
                "tags": n.tags,
                "summary": n.summary,
                "degree": deg,
            })
        })
        .collect();

    OnboardSections {
        project,
        layers,
        key_concepts,
        tour,
        file_map,
        hotspots,
    }
}

fn build_layer_lookup(layers: &[crate::commands::code_wiki_architecture::Layer]) -> std::collections::HashMap<String, String> {
    let mut out: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for l in layers {
        for nid in &l.node_ids {
            out.entry(nid.clone()).or_insert_with(|| l.name.clone());
        }
    }
    out
}

fn compute_degrees(graph: &KnowledgeGraph) -> std::collections::HashMap<String, u32> {
    let mut out: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for e in &graph.edges {
        *out.entry(e.source.clone()).or_insert(0) += 1;
        *out.entry(e.target.clone()).or_insert(0) += 1;
    }
    out
}

fn render_template(s: &OnboardSections) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {} — Onboarding\n\n", s.project["name"].as_str().unwrap_or("Project")));
    let descr = s.project["description"].as_str().unwrap_or("");
    if !descr.is_empty() {
        out.push_str(&format!("{}\n\n", descr));
    }
    let langs: Vec<String> = s.project["languages"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let frameworks: Vec<String> = s.project["frameworks"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if !langs.is_empty() || !frameworks.is_empty() {
        out.push_str(&format!(
            "_Languages:_ {} | _Frameworks:_ {}\n\n",
            langs.join(", "),
            frameworks.join(", ")
        ));
    }

    out.push_str("## Project Overview\n\n");
    if let Some(hash) = s.project["gitCommitHash"].as_str() {
        if !hash.is_empty() {
            out.push_str(&format!("_Commit:_ `{}`\n\n", hash));
        }
    }

    out.push_str("## Architecture Layers\n\n");
    if s.layers.is_empty() {
        out.push_str("_No layers defined._\n\n");
    } else {
        for l in &s.layers {
            let name = l["name"].as_str().unwrap_or("(unnamed)");
            let descr = l["description"].as_str().unwrap_or("");
            let count = l["nodeCount"].as_u64().unwrap_or(0);
            out.push_str(&format!(
                "### {name} ({count} nodes)\n\n{descr}\n\n"
            ));
        }
    }

    out.push_str("## Key Concepts\n\n");
    if s.key_concepts.is_empty() {
        out.push_str("_No key concepts detected (no file nodes with ≥3 tags)._\n\n");
    } else {
        for c in &s.key_concepts {
            let name = c["name"].as_str().unwrap_or("(unnamed)");
            let path = c["filePath"].as_str().unwrap_or("");
            let tags: Vec<String> = c["tags"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let summary = c["summary"].as_str().unwrap_or("");
            out.push_str(&format!(
                "- **{name}** (`{path}`) — {summary} _[tags: {}]_\n",
                tags.join(", ")
            ));
        }
        out.push('\n');
    }

    out.push_str("## Guided Tour\n\n");
    if s.tour.is_empty() {
        out.push_str("_No tour defined._\n\n");
    } else {
        for t in &s.tour {
            let order = t["order"].as_u64().unwrap_or(0);
            let title = t["title"].as_str().unwrap_or("(step)");
            let descr = t["description"].as_str().unwrap_or("");
            out.push_str(&format!("### Step {} — {}\n\n{}\n\n", order, title, descr));
        }
    }

    out.push_str("## File Map\n\n");
    if s.file_map.is_empty() {
        out.push_str("_No file-level nodes._\n\n");
    } else {
        for grp in &s.file_map {
            let layer = grp["layer"].as_str().unwrap_or("Unassigned");
            out.push_str(&format!("### Layer: {layer}\n\n"));
            let files = grp["files"].as_array().cloned().unwrap_or_default();
            for f in &files {
                let path = f["filePath"].as_str().unwrap_or("");
                let name = f["name"].as_str().unwrap_or("");
                let summary = f["summary"].as_str().unwrap_or("");
                let complexity = f["complexity"].as_str().unwrap_or("moderate");
                out.push_str(&format!(
                    "- `{path}` ({name}, _complexity: {complexity}_) — {summary}\n"
                ));
            }
            out.push('\n');
        }
    }

    out.push_str("## Complexity Hotspots\n\n");
    if s.hotspots.is_empty() {
        out.push_str("_No complexity=complex nodes._\n\n");
    } else {
        for h in &s.hotspots {
            let name = h["name"].as_str().unwrap_or("(unnamed)");
            let path = h["filePath"].as_str().unwrap_or("");
            let degree = h["degree"].as_u64().unwrap_or(0);
            out.push_str(&format!(
                "- **{name}** (`{path}`) — degree {degree} (most-connected complex nodes; approach with care)\n"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::ProjectMeta;
    use crate::commands::code_wiki_architecture::Layer;

    fn file_node(id: &str, file_path: &str, tags: Vec<String>, complexity: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            kind: "file".to_string(),
            name: file_path.split('/').last().unwrap_or(file_path).to_string(),
            file_path: file_path.to_string(),
            summary: format!("summary for {file_path}"),
            tags,
            complexity: complexity.to_string(),
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

    fn small_graph() -> KnowledgeGraph {
        KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec!["rust".to_string()],
                frameworks: vec!["tokio".to_string()],
                description: "demo project".to_string(),
                analyzed_at: "2026-07-01T00:00:00.000Z".to_string(),
                git_commit_hash: "deadbeef".to_string(),
            },
            nodes: vec![
                file_node(
                    "file:src/a.ts",
                    "src/a.ts",
                    vec!["auth".to_string(), "jwt".to_string(), "middleware".to_string()],
                    "complex",
                ),
                file_node(
                    "file:src/b.ts",
                    "src/b.ts",
                    vec!["db".to_string()],
                    "moderate",
                ),
                file_node(
                    "file:src/c.ts",
                    "src/c.ts",
                    vec!["auth".to_string(), "jwt".to_string(), "core".to_string(), "util".to_string()],
                    "complex",
                ),
            ],
            edges: vec![
                e("file:src/a.ts", "file:src/b.ts", "imports"),
                e("file:src/c.ts", "file:src/b.ts", "imports"),
                e("file:src/c.ts", "file:src/a.ts", "imports"),
            ],
            layers: vec![Layer {
                id: "layer:auth".to_string(),
                name: "Auth".to_string(),
                description: "auth".to_string(),
                node_ids: vec!["file:src/a.ts".to_string(), "file:src/c.ts".to_string()],
            }],
            tour: vec![TourStep {
                order: 1,
                title: "Start".to_string(),
                description: "begin here".to_string(),
                node_ids: vec!["file:src/a.ts".to_string()],
            }],
        }
    }

    #[test]
    fn assemble_sections_collects_six_inputs() {
        let g = small_graph();
        let s = assemble_sections(&g);
        assert_eq!(s.layers.len(), 1);
        assert_eq!(s.layers[0]["name"], "Auth");
        assert_eq!(s.key_concepts.len(), 2, "two file nodes with >=3 tags");
        assert_eq!(s.tour.len(), 1);
        assert_eq!(s.file_map.len(), 2, "Auth + Unassigned");
        assert_eq!(s.hotspots.len(), 2, "two complex nodes");
    }

    #[test]
    fn template_contains_all_six_headings() {
        let g = small_graph();
        let s = assemble_sections(&g);
        let out = render_template(&s);
        for h in [
            "# demo — Onboarding",
            "## Project Overview",
            "## Architecture Layers",
            "## Key Concepts",
            "## Guided Tour",
            "## File Map",
            "## Complexity Hotspots",
        ] {
            assert!(out.contains(h), "missing section {h} in:\n{out}");
        }
    }

    #[test]
    fn template_falls_back_when_no_layers() {
        let mut g = small_graph();
        g.layers.clear();
        let s = assemble_sections(&g);
        let out = render_template(&s);
        assert!(out.contains("No layers defined"));
    }

    #[test]
    fn template_handles_empty_concepts_and_hotspots() {
        let mut g = small_graph();
        g.nodes.clear();
        g.edges.clear();
        let s = assemble_sections(&g);
        let out = render_template(&s);
        assert!(out.contains("No key concepts"));
        assert!(out.contains("No complexity=complex"));
    }

    #[test]
    fn layer_lookup_maps_to_first_matching_layer() {
        let g = small_graph();
        let m = build_layer_lookup(&g.layers);
        assert_eq!(m.get("file:src/a.ts").map(String::as_str), Some("Auth"));
        assert!(m.get("file:src/b.ts").is_none());
    }

    #[test]
    fn degrees_count_incident_edges() {
        let g = small_graph();
        let d = compute_degrees(&g);
        // b.ts has 2 incoming; a.ts and c.ts each have 1 incoming + 1 outgoing = 2
        assert_eq!(d.get("file:src/b.ts").copied(), Some(2));
    }
}

use crate::commands::code_wiki_pipeline::GraphEdge;