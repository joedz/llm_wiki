// Knowledge-base graph builder for `/understand-knowledge`.
//
// Pipelines a Karpathy-style LLM wiki (markdown files under
// `raw/knowledge/<repo>/`) into the same on-disk `KnowledgeGraph`
// shape used for code wikis — but with `kind = "knowledge"` and
// node/edge kinds drawn from the wiki taxonomy:
//
//   Nodes: article / entity / topic / claim / source
//   Edges: cites / categorized_under / builds_on / contradicts /
//          exemplifies / authored_by
//
// Architecture mirror (UA's `/understand-knowledge` skill):
//   1. deterministic parse  — article / topic / claim / source
//                             + `cites` + `categorized_under` edges.
//   2. article-analyzer LLM — 10-15 article batches; the LLM
//                             emits `entity` / new `claim` nodes
//                             and implicit relationships.
//   3. assemble            — dedup by ID, drop dangling LLM edges.
//   4. save                — `wiki/knowledge/<repo>/knowledge-graph.json`
//                             plus `meta.json`. Emits a dedicated
//                             `codewiki-knowledge-done` event so
//                             the TS store doesn't have to share
//                             a generic `PipelineSummary`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::commands::code_wiki_pipeline::{
    GraphEdge, GraphNode, LlmRequestSpec, ProjectMeta,
};
use crate::commands::code_wiki_save::{write_atomic, write_graph_streaming};
use crate::commands::code_wiki_architecture::Layer;
use crate::commands::code_wiki_tour::TourStep;
use crate::commands::code_wiki_scanner::ScanResult;
use crate::commands::code_wiki::{knowledge_repo_root, knowledge_source_dir_for};
use crate::llm_client::{call_llm, LlmRequest, LlmResponse};

const ARTICLE_ANALYZER_PROMPT: &str = include_str!("../prompts/article_analyzer.md");
const INDEX_FILE_NAME: &str = "index.md";
const LOG_FILE_NAME: &str = "log.md";
const KNOW_META_FILE: &str = "meta.json";

// ============================================================================
// Section 1. Parser types and helpers
// ============================================================================

#[derive(Debug, Clone)]
pub struct WikiArticle {
    pub rel_path: String,
    pub stem: String,
    pub title: String,
    pub summary: String,
    pub headings: Vec<(u32, String, [u32; 2])>,
    pub wikilinks: Vec<(String, Option<String>)>,
    pub body: String,
}

#[derive(Debug, Default, Clone)]
pub struct KnowledgeParseResult {
    pub articles: Vec<WikiArticle>,
    pub topics: Vec<TopicEntry>,
    pub sources: Vec<SourceEntry>,
    pub topics_by_article: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct TopicEntry {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub id: String,
    pub name: String,
    pub rel_path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ScannedKbFile {
    pub rel_path: String,
    pub content: String,
    pub size_bytes: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct KnowledgeEnrichments {
    #[serde(default)]
    pub nodes: Vec<KnowledgeNodeEnrichment>,
    #[serde(default)]
    pub edges: Vec<KnowledgeEdgeEnrichment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNodeEnrichment {
    pub id: String,
    #[serde(rename = "type", default = "default_kind")]
    pub kind: String,
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_simple")]
    pub complexity: String,
}

fn default_kind() -> String {
    "entity".to_string()
}
fn default_simple() -> String {
    "simple".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEdgeEnrichment {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default = "default_edge_weight")]
    pub weight: f32,
    #[serde(default)]
    pub description: String,
}

fn default_edge_weight() -> f32 {
    0.5
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KnowledgeStats {
    pub articles: u32,
    pub claims: u32,
    pub entities: u32,
    pub topics: u32,
    pub sources: u32,
    pub edges: u32,
    pub wikilinks_unresolved: u32,
    pub llm_failures: u32,
}

impl KnowledgeStats {
    pub fn from_graph(graph: &KnowledgeGraphPartial, unresolved: u32, llm_failures: u32) -> Self {
        let mut stats = KnowledgeStats {
            articles: 0, claims: 0, entities: 0, topics: 0, sources: 0,
            edges: graph.edges.len() as u32,
            wikilinks_unresolved: unresolved,
            llm_failures,
        };
        for n in &graph.nodes {
            match n.kind.as_str() {
                "article" => stats.articles += 1,
                "claim" => stats.claims += 1,
                "entity" => stats.entities += 1,
                "topic" => stats.topics += 1,
                "source" => stats.sources += 1,
                _ => {}
            }
        }
        stats
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeGraphPartial {
    pub version: String,
    pub kind: String,
    pub project: ProjectMeta,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub layers: Vec<Layer>,
    pub tour: Vec<TourStep>,
}

// ============================================================================
// Section 2. Parse helpers
// ============================================================================

pub fn parse_article(content: &str, rel_path: String, stem: String) -> Option<WikiArticle> {
    let mut title: Option<String> = None;
    let mut summary = String::new();
    let mut headings: Vec<(u32, String, [u32; 2])> = Vec::new();
    let mut wikilinks: Vec<(String, Option<String>)> = Vec::new();

    let wikilink_re = Regex::new(r"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]").unwrap();

    let mut in_code_fence = false;
    let mut non_empty_seen = false;
    let mut first_para_done = false;
    let mut current_heading_level: Option<u32> = None;

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = (idx + 1) as u32;
        let line = raw_line;

        if line.starts_with("```") || line.starts_with("~~~") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            continue;
        }

        if title.is_none() {
            if line.starts_with("# ") || line.starts_with("#\t") {
                let trimmed = line.trim_start_matches('#').trim_start();
                title = Some(trimmed.to_string());
                current_heading_level = Some(1);
                headings.push((1, trimmed.to_string(), [line_no, line_no]));
                continue;
            }
        }

        let mut heading_match = None;
        if line.starts_with("# ") || line.starts_with("#\t") || line == "#" {
            heading_match = Some((1u32, line[1..].trim().to_string()));
        } else if line.starts_with("## ") || line.starts_with("##\t") {
            heading_match = Some((2, line[2..].trim().to_string()));
        } else if line.starts_with("### ") {
            heading_match = Some((3, line[3..].trim().to_string()));
        } else if line.starts_with("#### ") {
            heading_match = Some((4, line[4..].trim().to_string()));
        }
        if let Some((level, text)) = heading_match {
            headings.push((level, text.clone(), [line_no, line_no]));
            current_heading_level = Some(level);
            continue;
        }

        for cap in wikilink_re.captures_iter(line) {
            let target = cap[1].trim().to_string();
            let display = cap.get(2).map(|m| m.as_str().trim().to_string());
            wikilinks.push((target, display));
        }

        if !first_para_done && !line.trim().is_empty() && current_heading_level.unwrap_or(0) <= 2 {
            if !summary.is_empty() {
                summary.push(' ');
            }
            summary.push_str(line.trim());
            non_empty_seen = true;
        } else if non_empty_seen && line.trim().is_empty() {
            first_para_done = true;
        }
    }

    let title = title.unwrap_or_else(|| stem.clone());

    Some(WikiArticle {
        rel_path,
        stem,
        title,
        summary: trim_summary(&summary),
        headings,
        wikilinks,
        body: content.to_string(),
    })
}

fn trim_summary(s: &str) -> String {
    let s = s.trim();
    if s.len() <= 280 {
        return s.to_string();
    }
    let cut = s[..280].rfind(' ').unwrap_or(280);
    format!("{}…", &s[..cut])
}

pub fn parse_topics_from_index(
    scan_files: &[ScannedKbFile],
) -> (Vec<TopicEntry>, HashMap<String, Vec<String>>) {
    let mut topics: Vec<TopicEntry> = Vec::new();
    let mut article_to_topics: HashMap<String, Vec<String>> = HashMap::new();

    for file in scan_files {
        if !file.rel_path.eq_ignore_ascii_case(INDEX_FILE_NAME) {
            continue;
        }
        let mut current_topic: Option<String> = None;
        for raw_line in file.content.lines() {
            let line = raw_line.trim_start_matches(' ');
            if let Some(rest) = line.strip_prefix("## ") {
                let name = rest.trim().to_string();
                if name.is_empty() {
                    current_topic = None;
                    continue;
                }
                let id = format!("topic:{}", slugify(&name));
                topics.push(TopicEntry {
                    id: id.clone(),
                    name: name.clone(),
                    description: format!("Category {name} defined in index.md."),
                });
                current_topic = Some(id);
            } else if !line.starts_with('#') && !line.is_empty() {
                let re = Regex::new(r"\[\[([^\]|]+)(?:\|[^\]]+)?\]\]").unwrap();
                for cap in re.captures_iter(line) {
                    let stem = cap[1].trim().to_string();
                    if let Some(tid) = current_topic.clone() {
                        article_to_topics.entry(stem).or_default().push(tid);
                    }
                }
            }
        }
        break;
    }

    (topics, article_to_topics)
}

fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            prev_dash = false;
        } else if c.is_whitespace() || c == '_' || c == '/' {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

pub fn scan_knowledge_base(root: &Path) -> Result<Vec<ScannedKbFile>, String> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return Err(format!("knowledge root is not a directory: {}", root.display()));
    }
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let abs = entry.path();
        let ext = abs.extension().and_then(|s| s.to_str()).unwrap_or("");
        if !matches!(ext, "md" | "markdown" | "mdx") {
            continue;
        }
        let rel = abs
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string().replace('\\', "/"))
            .unwrap_or_else(|_| abs.to_string_lossy().to_string());
        if rel.eq_ignore_ascii_case(LOG_FILE_NAME) {
            continue;
        }
        if rel.to_ascii_lowercase().ends_with("claude.md")
            || rel.to_ascii_lowercase().ends_with("agents.md")
            || rel.starts_with("raw/")
        {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let content = std::fs::read_to_string(abs).unwrap_or_default();
        out.push(ScannedKbFile {
            rel_path: rel,
            content,
            size_bytes: size,
        });
    }
    Ok(out)
}

fn file_stem(rel_path: &str) -> String {
    let trimmed = rel_path.rsplit('/').next().unwrap_or(rel_path);
    let trimmed = trimmed.rsplit_once('.').map(|(s, _)| s).unwrap_or(trimmed);
    trimmed.to_string()
}

pub fn deterministic_parse(scan_files: &[ScannedKbFile]) -> KnowledgeParseResult {
    let (topics, topics_by_article) = parse_topics_from_index(scan_files);
    let mut articles = Vec::new();
    let mut sources = Vec::new();

    for f in scan_files {
        if f.rel_path.eq_ignore_ascii_case(INDEX_FILE_NAME) {
            continue;
        }
        if f.rel_path.starts_with("raw/") {
            let stem = f.rel_path.trim_start_matches("raw/").to_string();
            sources.push(SourceEntry {
                id: format!("source:{stem}"),
                name: f.rel_path.clone(),
                rel_path: f.rel_path.clone(),
                size_bytes: f.size_bytes,
            });
            continue;
        }
        let stem = file_stem(&f.rel_path);
        if let Some(article) = parse_article(&f.content, f.rel_path.clone(), stem) {
            articles.push(article);
        }
    }

    KnowledgeParseResult {
        articles,
        topics,
        sources,
        topics_by_article,
    }
}

pub fn validate_wikilinks(
    articles: &[WikiArticle],
) -> (Vec<(String, String, Option<String>)>, Vec<String>) {
    let known_stems: HashSet<String> = articles.iter().map(|a| a.stem.clone()).collect();
    let mut edges = Vec::new();
    let mut warnings = Vec::new();
    for article in articles {
        for (target, display) in &article.wikilinks {
            if known_stems.contains(target) {
                let edge_id_target = format!("article:{}", target);
                let edge_id_source = format!("article:{}", article.stem);
                edges.push((edge_id_source, edge_id_target, display.clone()));
            } else {
                warnings.push(format!(
                    "wikilink in `{}` → `{}` not found",
                    article.rel_path, target
                ));
            }
        }
    }
    (edges, warnings)
}

fn line_count_for_complexity(content: &str) -> String {
    let lines = content.lines().filter(|l| !l.trim().is_empty()).count();
    if lines < 50 {
        "simple".to_string()
    } else if lines < 250 {
        "moderate".to_string()
    } else {
        "complex".to_string()
    }
}

pub fn emit_base_knowledge_graph(
    parse_result: &KnowledgeParseResult,
    project_name: &str,
    description: &str,
    git_commit_hash: &str,
) -> KnowledgeGraphPartial {
    let now = chrono::Utc::now().to_rfc3339();
    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut topic_to_nodes: HashMap<String, Vec<String>> = HashMap::new();

    for t in &parse_result.topics {
        nodes.push(GraphNode {
            id: t.id.clone(),
            kind: "topic".to_string(),
            name: t.name.clone(),
            file_path: "index.md".into(),
            summary: t.description.clone(),
            tags: vec!["topic".to_string()],
            complexity: "moderate".to_string(),
            location: None,
            language_notes: None,
        });
    }

    for (article_stem, topic_ids) in &parse_result.topics_by_article {
        for tid in topic_ids {
            let article_id = format!("article:{article_stem}");
            topic_to_nodes.entry(tid.clone()).or_default().push(article_id.clone());
            edges.push(GraphEdge {
                source: article_id,
                target: tid.clone(),
                kind: "categorized_under".to_string(),
                direction: "forward".to_string(),
                weight: 1.0,
                ..Default::default()
            });
        }
    }

    for a in &parse_result.articles {
        let article_id = format!("article:{}", a.stem);
        nodes.push(GraphNode {
            id: article_id.clone(),
            kind: "article".to_string(),
            name: a.title.clone(),
            file_path: a.rel_path.clone(),
            summary: a.summary.clone(),
            tags: vec!["article".to_string()],
            complexity: line_count_for_complexity(&a.body),
            location: None,
            language_notes: None,
        });

        for (level, htext, line_range) in &a.headings {
            if *level < 2 {
                continue;
            }
            let claim_id = format!("claim:{}:{}", a.stem, slugify(htext));
            nodes.push(GraphNode {
                id: claim_id.clone(),
                kind: "claim".to_string(),
                name: htext.clone(),
                file_path: a.rel_path.clone(),
                summary: format!("Claim from {} §{}", a.title, htext),
                tags: vec!["claim".to_string()],
                complexity: "simple".to_string(),
                location: Some(crate::commands::code_wiki_pipeline::NodeLocation {
                    start_line: line_range[0],
                    end_line: line_range[1],
                }),
                language_notes: None,
            });
            edges.push(GraphEdge {
                source: claim_id,
                target: article_id.clone(),
                kind: "cites".to_string(),
                direction: "forward".to_string(),
                weight: 0.8,
                ..Default::default()
            });
        }
    }

    for s in &parse_result.sources {
        nodes.push(GraphNode {
            id: s.id.clone(),
            kind: "source".to_string(),
            name: s.name.clone(),
            file_path: s.rel_path.clone(),
            summary: format!("Raw source file ({} bytes)", s.size_bytes),
            tags: vec!["source".to_string()],
            complexity: "simple".to_string(),
            location: None,
            language_notes: None,
        });
    }

    let (cites_edges, _unresolved) = validate_wikilinks(&parse_result.articles);
    for (source, target, _display) in cites_edges {
        edges.push(GraphEdge {
            source,
            target,
            kind: "cites".to_string(),
            direction: "forward".to_string(),
            weight: 0.7,
            ..Default::default()
        });
    }

    KnowledgeGraphPartial {
        version: "1.0.0".to_string(),
        kind: "knowledge".to_string(),
        project: ProjectMeta {
            name: project_name.to_string(),
            languages: vec!["markdown".to_string()],
            frameworks: vec![],
            description: description.to_string(),
            analyzed_at: now,
            git_commit_hash: git_commit_hash.to_string(),
        },
        nodes,
        edges,
        layers: vec![],
        tour: vec![],
    }
}

pub fn apply_llm_enrichments(
    graph: &mut KnowledgeGraphPartial,
    enrichments: &KnowledgeEnrichments,
    warnings: &mut Vec<String>,
) {
    let mut current_ids: HashSet<String> = graph.nodes.iter().map(|n| n.id.clone()).collect();
    for ent in enrichments.nodes.iter() {
        let kind_norm = match ent.kind.as_str() {
            "entity" | "claim" | "topic" | "source" | "article" => ent.kind.clone(),
            other => {
                warnings.push(format!("LLM enrichment unknown kind `{}`; defaulting to entity", other));
                "entity".to_string()
            }
        };

        let id = if ent.id.starts_with(&format!("{}:", kind_norm)) {
            ent.id.clone()
        } else {
            format!("{}:{}", kind_norm, slugify(&ent.id))
        };

        if current_ids.contains(&id) {
            continue;
        }
        graph.nodes.push(GraphNode {
            id: id.clone(),
            kind: kind_norm.clone(),
            name: ent.name.clone(),
            file_path: String::new(),
            summary: ent.summary.clone(),
            tags: {
                let mut t = vec![kind_norm.clone()];
                t.extend(ent.tags.iter().cloned());
                t
            },
            complexity: if ent.complexity.is_empty() {
                "simple".to_string()
            } else {
                ent.complexity.clone()
            },
            location: None,
            language_notes: None,
        });
        current_ids.insert(id.clone());

        if kind_norm == "claim" {
            if let Some(rest) = id.strip_prefix("claim:") {
                if let Some((stem, _)) = rest.split_once(':') {
                    let article_id = format!("article:{}", stem);
                    if graph.nodes.iter().any(|n| n.id == article_id) {
                        graph.edges.push(GraphEdge {
                            source: id.clone(),
                            target: article_id,
                            kind: "cites".to_string(),
                            direction: "forward".to_string(),
                            weight: 0.7,
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }

    for e in &enrichments.edges {
        if !current_ids.contains(&e.source) || !current_ids.contains(&e.target) {
            warnings.push(format!(
                "LLM edge {} -> {} (type {}) dropped — endpoint not found",
                e.source, e.target, e.kind
            ));
            continue;
        }
        graph.edges.push(GraphEdge {
            source: e.source.clone(),
            target: e.target.clone(),
            kind: e.kind.clone(),
            direction: "forward".to_string(),
            weight: e.weight,
            ..Default::default()
        });
    }
}

pub fn batch_articles(articles: &[WikiArticle], size: usize) -> Vec<Vec<WikiArticle>> {
    if size == 0 {
        return vec![articles.to_vec()];
    }
    articles.chunks(size).map(|c| c.to_vec()).collect()
}

pub fn parse_enrichment_response(
    content: &str,
    _expected_articles: usize,
) -> Result<KnowledgeEnrichments, String> {
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
    let enr: KnowledgeEnrichments = serde_json::from_value(parsed.clone())
        .map_err(|e| format!("enrichment shape invalid: {e}"))?;
    Ok(enr)
}

pub async fn call_enrichment_llm(
    llm_request: &LlmRequestSpec,
    articles: &[&WikiArticle],
) -> Result<KnowledgeEnrichments, String> {
    let system = ARTICLE_ANALYZER_PROMPT.to_string();
    let mut user = String::new();
    user.push_str("Articles in this batch:\n\n");
    for a in articles {
        user.push_str(&format!(
            "---\npath: {}\ntitle: {}\nsummary: {}\n",
            a.rel_path, a.title, a.summary
        ));
    }
    user.push_str(
        "\nFor each article emit entity / claim nodes and implicit\n\
         edges using the schema described in your system prompt.\n",
    );
    let mut req: LlmRequest = llm_request.into_request(system, user);
    req.temperature = 0.3;
    let resp: LlmResponse = call_llm(req, 1)
        .await
        .map_err(|e| format!("LLM call failed: {e:?}"))?;
    parse_enrichment_response(&resp.content, articles.len())
}

pub fn save_knowledge_graph(
    graph: &KnowledgeGraphPartial,
    project_path: &Path,
    repo_name: &str,
) -> Result<(PathBuf, PathBuf), String> {
    use crate::commands::code_wiki::{
        knowledge_graph_path_for, knowledge_meta_path_for,
    };
    let repo_dir = knowledge_repo_root(project_path, repo_name);
    std::fs::create_dir_all(&repo_dir).map_err(|e| format!("mkdir wiki/knowledge: {e}"))?;
    let graph_path = knowledge_graph_path_for(project_path, repo_name);
    let bytes = serde_json::to_vec_pretty(graph)
        .map_err(|e| format!("serialize knowledge graph: {e}"))?;
    write_atomic(&graph_path, &bytes).map_err(|e| format!("write knowledge graph: {e}"))?;
    let meta_path = knowledge_meta_path_for(project_path, repo_name);
    let meta = json!({
        "lastAnalyzedAt": chrono::Utc::now().to_rfc3339(),
        "version": "knowledgegraph-1.0.0",
        "kind": "knowledge",
        "repo": repo_name,
    });
    let meta_bytes = serde_json::to_vec_pretty(&meta)
        .map_err(|e| format!("serialize meta: {e}"))?;
    write_atomic(&meta_path, &meta_bytes).map_err(|e| format!("write meta: {e}"))?;
    Ok((graph_path, meta_path))
}

pub fn load_knowledge_graph(
    project_path: &Path,
    repo_name: &str,
) -> Result<Option<KnowledgeGraphPartial>, String> {
    use crate::commands::code_wiki::knowledge_graph_path_for;
    let graph_path = knowledge_graph_path_for(project_path, repo_name);
    if !graph_path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&graph_path).map_err(|e| format!("read graph: {e}"))?;
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("graph not valid JSON: {e}"))?;
    if parsed.get("kind").and_then(|v| v.as_str()) != Some("knowledge") {
        return Err(format!(
            "graph at {} has kind={:?}, expected \"knowledge\"",
            graph_path.display(),
            parsed.get("kind")
        ));
    }
    let partial: KnowledgeGraphPartial = serde_json::from_value(parsed)
        .map_err(|e| format!("deserialize knowledge graph: {e}"))?;
    Ok(Some(partial))
}

pub fn scan_result_for_knowledge(files: &[ScannedKbFile]) -> ScanResult {
    use crate::commands::code_wiki_scanner::{ScanResult as SR, ScanStats, ScannedFile};
    let mut by_language = BTreeMap::new();
    by_language.insert("markdown".to_string(), files.len() as u32);
    let scanned: Vec<ScannedFile> = files
        .iter()
        .map(|f| ScannedFile {
            path: f.rel_path.clone(),
            language: "markdown".to_string(),
            size_lines: f.content.lines().count() as u32,
            file_category: "docs".to_string(),
        })
        .collect();
    SR {
        project_root: String::new(),
        files: scanned,
        total_files: files.len() as u32,
        filtered_by_ignore: 0,
        estimated_complexity: "moderate".to_string(),
        stats: ScanStats {
            files_scanned: files.len() as u32,
            by_category: BTreeMap::new(),
            by_language,
        },
        project_name: String::new(),
        project_description: format!("Knowledge base with {} articles", files.len()),
        frameworks: Vec::new(),
        git_commit_hash: String::new(),
    }
}

pub fn emit_done_batch(
    app: &AppHandle,
    pipeline_id: &str,
    phase: u32,
    batch_index: u32,
    total_batches: u32,
) {
    let _ = app.emit(
        "codewiki-knowledge-progress",
        KnowledgeProgress::Batch {
            pipeline_id: pipeline_id.to_string(),
            phase,
            batch_index,
            total_batches,
            file_count: 0,
            status: "done".to_string(),
        },
    );
}

// ============================================================================
// Section 3. Pipeline orchestrator + Tauri command
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeRunSummary {
    pub pipeline_id: String,
    pub project_path: String,
    pub repo_name: String,
    pub final_graph_path: String,
    pub final_meta_path: String,
    pub node_count: u32,
    pub edge_count: u32,
    pub kind: String,
    pub duration_ms: u64,
    pub warnings: Vec<String>,
    pub stats: KnowledgeStatsWire,
}

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeStatsWire {
    pub articles: u32,
    pub claims: u32,
    pub entities: u32,
    pub topics: u32,
    pub sources: u32,
    pub edges: u32,
    pub wikilinks_unresolved: u32,
    pub llm_failures: u32,
}

impl From<KnowledgeStats> for KnowledgeStatsWire {
    fn from(s: KnowledgeStats) -> Self {
        KnowledgeStatsWire {
            articles: s.articles,
            claims: s.claims,
            entities: s.entities,
            topics: s.topics,
            sources: s.sources,
            edges: s.edges,
            wikilinks_unresolved: s.wikilinks_unresolved,
            llm_failures: s.llm_failures,
        }
    }
}

/// Knowledge-specific progress events on their own channel.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum KnowledgeProgress {
    Started {
        #[serde(rename = "pipelineId")]
        pipeline_id: String,
        #[serde(rename = "repoName")]
        repo_name: String,
        #[serde(rename = "totalPhases")]
        total_phases: u32,
    },
    Phase {
        #[serde(rename = "pipelineId")]
        pipeline_id: String,
        phase: u32,
        label: String,
        status: String,
    },
    Batch {
        #[serde(rename = "pipelineId")]
        pipeline_id: String,
        phase: u32,
        #[serde(rename = "batchIndex")]
        batch_index: u32,
        #[serde(rename = "totalBatches")]
        total_batches: u32,
        #[serde(rename = "fileCount")]
        #[serde(default)]
        file_count: u32,
        status: String,
    },
}

/// Tauri command: run the knowledge pipeline in background.
#[tauri::command]
pub async fn code_wiki_run_knowledge_pipeline(
    project_path: String,
    repo_name: String,
    llm: Option<LlmRequestSpec>,
    app: AppHandle,
) -> Result<(), String> {
    let result = run_knowledge(app.clone(), project_path, repo_name, llm).await;
    if let Err(e) = &result {
        eprintln!("[code-wiki knowledge pipeline] run failed: {e}");
    }
    result
}

pub async fn run_knowledge(
    app: AppHandle,
    project_path: String,
    repo_name: String,
    llm: Option<LlmRequestSpec>,
) -> Result<(), String> {
    let started = std::time::Instant::now();
    let pipeline_id = project_path.clone();
    let project_root = PathBuf::from(&project_path);
    let source_dir = knowledge_source_dir_for(&project_root, &repo_name);
    if !source_dir.is_dir() {
        return Err(format!(
            "knowledge source dir not found: {} (create raw/knowledge/{}/ first)",
            source_dir.display(),
            repo_name
        ));
    }
    let _ = std::fs::create_dir_all(knowledge_repo_root(&project_root, &repo_name))
        .map_err(|e| format!("mkdir wiki/knowledge: {e}"))?;

    let _ = app.emit(
        "codewiki-knowledge-progress",
        KnowledgeProgress::Started {
            pipeline_id: pipeline_id.clone(),
            repo_name: repo_name.clone(),
            total_phases: 3,
        },
    );

    let mut warnings: Vec<String> = Vec::new();

    let _ = app.emit(
        "codewiki-knowledge-progress",
        KnowledgeProgress::Phase {
            pipeline_id: pipeline_id.clone(),
            phase: 0,
            label: "Scan + parse".to_string(),
            status: "running".to_string(),
        },
    );
    let scan_files: Vec<ScannedKbFile> =
        scan_knowledge_base(&source_dir).map_err(|e| format!("scan knowledge base: {e}"))?;
    if scan_files.is_empty() {
        return Err(format!(
            "no markdown files found under {}",
            source_dir.display()
        ));
    }
    let parse_result: KnowledgeParseResult = deterministic_parse(&scan_files);
    let mut graph: KnowledgeGraphPartial =
        emit_base_knowledge_graph(&parse_result, &repo_name, "", "");
    let _ = app.emit(
        "codewiki-knowledge-progress",
        KnowledgeProgress::Phase {
            pipeline_id: pipeline_id.clone(),
            phase: 0,
            label: "Scan + parse".to_string(),
            status: "done".to_string(),
        },
    );

    let mut llm_failures: u32 = 0;
    if let Some(llm_spec) = llm {
        let articles_for_batches: Vec<WikiArticle> = parse_result.articles.clone();
        let batches = batch_articles(&articles_for_batches, 12);
        let total_batches = batches.len() as u32;
        let _ = app.emit(
            "codewiki-knowledge-progress",
            KnowledgeProgress::Phase {
                pipeline_id: pipeline_id.clone(),
                phase: 1,
                label: format!("Enrich {total_batches} batches via LLM"),
                status: "running".to_string(),
            },
        );
        for (idx, batch) in batches.iter().enumerate() {
            match call_enrichment_llm(&llm_spec, &batch.iter().collect::<Vec<_>>()).await {
                Ok(enr) => {
                    let mut warn_buf = Vec::new();
                    apply_llm_enrichments(&mut graph, &enr, &mut warn_buf);
                    warnings.extend(warn_buf);
                }
                Err(e) => {
                    llm_failures += 1;
                    warnings.push(format!("LLM batch {idx} failed: {e}"));
                }
            }
            emit_done_batch(&app, &pipeline_id, 1, idx as u32, total_batches);
        }
        let _ = app.emit(
            "codewiki-knowledge-progress",
            KnowledgeProgress::Phase {
                pipeline_id: pipeline_id.clone(),
                phase: 1,
                label: format!("Enrich {total_batches} batches via LLM"),
                status: "done".to_string(),
            },
        );
    } else {
        let _ = app.emit(
            "codewiki-knowledge-progress",
            KnowledgeProgress::Phase {
                pipeline_id: pipeline_id.clone(),
                phase: 1,
                label: "Enrich (no LLM)".to_string(),
                status: "done".to_string(),
            },
        );
    }

    let _ = app.emit(
        "codewiki-knowledge-progress",
        KnowledgeProgress::Phase {
            pipeline_id: pipeline_id.clone(),
            phase: 2,
            label: "Save".to_string(),
            status: "running".to_string(),
        },
    );
    let (graph_path, meta_path) = save_knowledge_graph(&graph, &project_root, &repo_name)?;
    let unresolved = warnings
        .iter()
        .filter(|w| w.contains("wikilink"))
        .count() as u32;
    let stats = KnowledgeStats::from_graph(&graph, unresolved, llm_failures);

    let summary = KnowledgeRunSummary {
        pipeline_id: pipeline_id.clone(),
        project_path,
        repo_name,
        final_graph_path: graph_path.to_string_lossy().to_string(),
        final_meta_path: meta_path.to_string_lossy().to_string(),
        node_count: graph.nodes.len() as u32,
        edge_count: graph.edges.len() as u32,
        kind: "knowledge".to_string(),
        duration_ms: started.elapsed().as_millis() as u64,
        warnings: warnings.clone(),
        stats: stats.into(),
    };
    let _ = app.emit(
        "codewiki-knowledge-progress",
        KnowledgeProgress::Phase {
            pipeline_id: pipeline_id.clone(),
            phase: 2,
            label: "Save".to_string(),
            status: "done".to_string(),
        },
    );
    let _ = app.emit("codewiki-knowledge-done", summary);
    Ok(())
}

/// Path helper re-export so external callers can resolve the
/// graph path without importing the long constant chain.
pub fn graph_path_for(project_path: &Path, repo_name: &str) -> std::path::PathBuf {
    use crate::commands::code_wiki::knowledge_graph_path_for;
    knowledge_graph_path_for(project_path, repo_name)
}

pub fn load(
    project_path: &Path,
    repo_name: &str,
) -> Result<Option<KnowledgeGraphPartial>, String> {
    load_knowledge_graph(project_path, repo_name)
}

pub fn to_scan_result(files: &[ScannedKbFile]) -> ScanResult {
    scan_result_for_knowledge(files)
}

// ============================================================================
// Section 4. Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_file(rel: &str, body: &str) -> ScannedKbFile {
        ScannedKbFile {
            rel_path: rel.to_string(),
            content: body.to_string(),
            size_bytes: body.len() as u64,
        }
    }

    #[test]
    fn wikilink_basic_extraction() {
        let content = "Before [[other-article|display]] middle\n\
            [[plain-link]] end.";
        let re = Regex::new(r"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]").unwrap();
        let links: Vec<(String, Option<String>)> = re
            .captures_iter(content)
            .map(|c| {
                (
                    c[1].trim().to_string(),
                    c.get(2).map(|m| m.as_str().trim().to_string()),
                )
            })
            .collect();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0], ("other-article".into(), Some("display".into())));
        assert_eq!(links[1], ("plain-link".into(), None));
    }

    #[test]
    fn slugify_handles_punct() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("Foo / Bar!"), "foo-bar");
        assert_eq!(slugify("inner --dash"), "inner-dash");
    }

    #[test]
    fn parse_article_extracts_h1_and_first_paragraph() {
        let content = "# Title here\n\nFirst paragraph one.\nSecond line of first.\n\nSecond paragraph.";
        let a = parse_article(content, "wiki/foo.md".into(), "foo".into()).unwrap();
        assert_eq!(a.title, "Title here");
        assert!(a.summary.contains("First paragraph"));
        assert!(a.summary.contains("Second line"));
    }

    #[test]
    fn emit_knowledge_graph_includes_articles_topics_and_edges() {
        let scan = vec![
            mk_file("index.md", "## Topic A\n\nIndex entry: [[foo]]\n\nMore: [[bar]]"),
            mk_file("wiki/foo.md", "# Foo\n\nFoo body with [[bar]]."),
            mk_file("wiki/bar.md", "# Bar\n\nBar body."),
        ];
        let parsed = deterministic_parse(&scan);
        let graph = emit_base_knowledge_graph(&parsed, "demo", "", "deadbeef");
        assert_eq!(graph.kind, "knowledge");
        let node_ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(node_ids.contains(&"article:foo"));
        assert!(node_ids.contains(&"article:bar"));
        assert!(node_ids.iter().any(|id| id.starts_with("topic:")));
        let has_cites = graph.edges.iter().any(|e| {
            e.kind == "cites"
                && e.source == "article:foo"
                && e.target == "article:bar"
        });
        assert!(has_cites, "expected cites edge foo -> bar, edges={:#?}", graph.edges);
        let has_cat = graph.edges.iter().any(|e| {
            e.kind == "categorized_under"
                && e.source == "article:foo"
                && e.target.starts_with("topic:")
        });
        assert!(has_cat, "expected categorized_under edge, edges={:#?}", graph.edges);
    }

    #[test]
    fn unresolved_wikilinks_are_warned_but_dropped() {
        let scan = vec![mk_file("wiki/foo.md", "# Foo\n\nlinks to [[nonexistent]]")];
        let parsed = deterministic_parse(&scan);
        let (edges, warns) = validate_wikilinks(&parsed.articles);
        assert!(edges.is_empty());
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("nonexistent"));
    }

    #[test]
    fn apply_llm_enrichments_dedupes_and_warns_on_dangling() {
        let scan = vec![
            mk_file("wiki/foo.md", "# Foo\n\nFoo body."),
            mk_file("wiki/bar.md", "# Bar\n\nBar body."),
        ];
        let parsed = deterministic_parse(&scan);
        let mut graph = emit_base_knowledge_graph(&parsed, "demo", "", "deadbeef");
        let enrichments = KnowledgeEnrichments {
            nodes: vec![KnowledgeNodeEnrichment {
                id: "entity:tok".into(),
                kind: "entity".into(),
                name: "Tok".into(),
                summary: "Tokenizer".into(),
                tags: vec!["ai".into()],
                complexity: "simple".into(),
            }],
            edges: vec![KnowledgeEdgeEnrichment {
                source: "article:foo".into(),
                target: "article:nonexistent".into(),
                kind: "builds_on".into(),
                weight: 0.5,
                description: "x".into(),
            }],
        };
        let mut warnings = Vec::new();
        apply_llm_enrichments(&mut graph, &enrichments, &mut warnings);
        assert!(graph.nodes.iter().any(|n| n.id == "entity:tok"));
        assert!(
            warnings.iter().any(|w| w.contains("dropped")),
            "expected dropped warning, got {warnings:?}"
        );
    }

    #[test]
    fn batch_articles_chunks_correctly() {
        let articles: Vec<WikiArticle> = (0..33)
            .map(|i| WikiArticle {
                rel_path: format!("wiki/{i}.md"),
                stem: format!("article-{i}"),
                title: format!("Article {i}"),
                summary: String::new(),
                headings: vec![],
                wikilinks: vec![],
                body: String::new(),
            })
            .collect();
        let batches = batch_articles(&articles, 10);
        assert_eq!(batches.len(), 4);
        assert_eq!(batches[0].len(), 10);
        assert_eq!(batches[3].len(), 3);
    }

    #[test]
    fn parse_enrichment_response_strips_code_fence() {
        let body = "```json\n{\"nodes\":[], \"edges\":[]}\n```";
        let enr = parse_enrichment_response(body, 12).expect("parse");
        assert!(enr.nodes.is_empty());
        assert!(enr.edges.is_empty());
    }
}
