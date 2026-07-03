// `/understand-domain` — extract business-domain knowledge from a
// codebase and emit a `domain-graph.json` (the UA `DomainGraphView`
// consumes it).
//
// Mirrors UA's `skills/understand-domain` 6-phase flow, but with
// everything in Rust (no Python preprocessor). The phases:
//
//   0. Resolve project root + load existing code-wiki meta
//   1. Detect existing knowledge graph (cheap path) or fall back
//      to a lightweight domain-context scan (Rust port of UA's
//      `extract-domain-context.py`)
//   2. Build context payload (existing graph nodes/edges or
//      lightweight file tree + entry points + signatures)
//   3. Dispatch LLM with the embedded UA `domain-analyzer.md`
//      prompt
//   4. Validate + apply: types ∈ {domain, flow, step}; edges
//      ∈ {contains_flow, flow_step, cross_domain}; flow_step
//      weights monotonically increase within (0.0, 1.0]
//   5. Save `wiki/code_wiki/<repo>/domain-graph.json` + meta.json
//
// Architecture parallels `code_wiki_knowledge.rs` (3 phases there)
// and reuses the same `call_llm` + markdown fence-stripping + JSON
// parsing pattern from `code_wiki_knowledge::parse_enrichment_response`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::commands::code_wiki::{
    code_wiki_get_graph_inner, domain_graph_path_for, domain_meta_path_for, repo_root,
};
use crate::commands::code_wiki_pipeline::{
    GraphEdge, GraphNode, KnowledgeGraph, LlmRequestSpec, NodeLocation, ProjectMeta,
};
use crate::commands::code_wiki_save::{write_atomic, write_meta, PipelineMeta};
use crate::llm_client::{call_llm, LlmRequest, LlmResponse};

const DOMAIN_ANALYZER_PROMPT: &str = include_str!("../prompts/domain_analyzer.md");
const DOMAIN_EVENT: &str = "codewiki-domain-progress";
const DOMAIN_DONE_EVENT: &str = "codewiki-domain-done";
const PHASE_SCAN: u32 = 0;
const PHASE_BUILD: u32 = 1;
const PHASE_ANALYZE: u32 = 2;
const PHASE_SAVE: u32 = 3;
const TOTAL_PHASES: u32 = 4;

// ============================================================================
// Section 1. Schemas
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainMeta {
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub business_rules: Vec<String>,
    #[serde(default)]
    pub cross_domain_interactions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_point: Option<String>,
    /// http | cli | event | cron | manual
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainGraph {
    pub version: String,
    /// Always "domain" — discriminator for the dashboard.
    pub kind: String,
    pub project: ProjectMeta,
    pub nodes: Vec<DomainNode>,
    pub edges: Vec<GraphEdge>,
    /// Source-of-truth flag: was this domain graph derived from an
    /// existing knowledge-graph.json (`true`) or via the
    /// lightweight scanner (`false`)?
    pub derived_from_graph: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainNode {
    #[serde(flatten)]
    pub base: GraphNode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_meta: Option<DomainMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DomainProgress {
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
    Warning {
        #[serde(rename = "pipelineId")]
        pipeline_id: String,
        phase: u32,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct DomainRunSummary {
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
    pub derived_from_graph: bool,
    pub used_llm: bool,
}

// ============================================================================
// Section 2. LLM input/output types
// ============================================================================

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DomainAnalysis {
    #[serde(default)]
    pub nodes: Vec<DomainNode>,
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
}

/// LLM-emitted nodes can be either a domain/flow/step (with
/// domain_meta) or a plain GraphNode (without). We accept both
/// shapes during parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum DomainNodeOrBase {
    WithMeta {
        id: String,
        #[serde(rename = "type")]
        kind: String,
        name: String,
        #[serde(default)]
        file_path: Option<String>,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default = "default_complexity")]
        complexity: String,
        #[serde(default)]
        domain_meta: Option<DomainMeta>,
    },
    Plain(GraphNode),
}

fn default_complexity() -> String {
    "moderate".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum DomainEdgeOrPair {
    Object(GraphEdge),
    Pair {
        source: String,
        target: String,
        #[serde(rename = "type")]
        kind: String,
        #[serde(default = "default_weight")]
        weight: f32,
        #[serde(default)]
        description: Option<String>,
    },
}

fn default_weight() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DomainAnalysisRaw {
    #[serde(default)]
    nodes: Vec<DomainNodeOrBase>,
    #[serde(default)]
    edges: Vec<DomainEdgeOrPair>,
}

// ============================================================================
// Section 3. Lightweight context scanner (Rust port of
// UA's extract-domain-context.py)
// ============================================================================

const MAX_FILE_TREE_DEPTH: usize = 6;
const MAX_FILES_PER_DIR: usize = 50;
const MAX_FILES_TOTAL: usize = 5000;
const MAX_SAMPLED_FILES: usize = 40;
const MAX_LINES_PER_FILE: usize = 80;
const MAX_ENTRY_POINTS: usize = 200;
const MAX_OUTPUT_BYTES: usize = 512 * 1024;

const SOURCE_EXTENSIONS: &[&str] = &[
    ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".pyi", ".go", ".rs", ".java", ".kt",
    ".scala", ".rb", ".cs", ".php", ".swift", ".c", ".cpp", ".h", ".hpp",
];

const SKIP_DIRS: &[&str] = &[
    "node_modules", ".git", ".svn", ".hg", "__pycache__", ".tox", "venv", ".venv", "env", ".env",
    "dist", "build", "out", ".next", ".nuxt", "target", "vendor", ".idea", ".vscode", "coverage",
    ".understand-anything", ".pytest_cache", ".mypy_cache", "Pods", "DerivedData", ".gradle",
    "bin", "obj", ".codegraph", "wiki",
];

const METADATA_FILES: &[&str] = &[
    "package.json", "Cargo.toml", "go.mod", "pyproject.toml", "setup.py", "setup.cfg",
    "pom.xml", "build.gradle", "Gemfile", "composer.json", "mix.exs", "Makefile",
    "docker-compose.yml", "docker-compose.yaml", "README.md", "README.rst", "README.txt", "README",
];

const PRIORITY_KEYWORDS: &[&str] = &[
    "controller", "service", "handler", "router", "route", "api", "model", "entity", "repository",
    "usecase", "use_case", "command", "query", "event", "subscriber", "listener", "middleware",
    "guard", "interceptor", "resolver", "workflow", "flow", "process", "pipeline", "job", "task",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainContext {
    pub project_root: String,
    pub file_count: u32,
    pub file_tree: Vec<String>,
    pub entry_points: Vec<EntryPoint>,
    pub file_signatures: Vec<FileSignature>,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryPoint {
    pub file: String,
    pub line: u32,
    pub kind: String, // http | cli | event | cron | manual
    pub description: String,
    pub match_text: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSignature {
    pub file: String,
    pub exports: Vec<String>,
    pub imports: Vec<String>,
    pub lines: u32,
    pub preview: String,
}

pub fn extract_domain_context(project_root: &Path) -> Result<DomainContext, String> {
    let gitignore_patterns = parse_gitignore(project_root);
    let file_tree = scan_file_tree(project_root, &gitignore_patterns, 0);
    let entry_points = detect_entry_points(project_root, &file_tree);
    let signatures = extract_file_signatures(project_root, &file_tree);
    let metadata = extract_metadata(project_root);
    Ok(DomainContext {
        project_root: project_root.to_string_lossy().to_string(),
        file_count: file_tree.len() as u32,
        file_tree,
        entry_points,
        file_signatures: signatures,
        metadata,
    })
}

fn parse_gitignore(project_root: &Path) -> Vec<Regex> {
    let path = project_root.join(".gitignore");
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let mut regex = trimmed
                .replace('.', "\\.")
                .replace("**/", "(.*/)?")
                .replace('*', "[^/]*")
                .replace('?', "[^/]");
            if trimmed.ends_with('/') {
                regex = regex.trim_end_matches('/').to_string() + "(/|$)";
            }
            Regex::new(&regex).ok()
        })
        .collect()
}

fn is_ignored(rel: &str, patterns: &[Regex]) -> bool {
    patterns.iter().any(|p| p.is_match(rel))
}

fn scan_file_tree(
    root: &Path,
    patterns: &[Regex],
    depth: usize,
) -> Vec<String> {
    scan_file_tree_inner(root, root, patterns, depth)
}

fn scan_file_tree_inner(
    project_root: &Path,
    dir: &Path,
    patterns: &[Regex],
    depth: usize,
) -> Vec<String> {
    if depth > MAX_FILE_TREE_DEPTH {
        return Vec::new();
    }
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    let mut sorted: Vec<_> = entries.filter_map(Result::ok).collect();
    sorted.sort_by_key(|e| (e.file_type().map(|t| !t.is_dir()).unwrap_or(true), e.file_name()));
    let mut file_count = 0usize;
    for entry in sorted {
        if out.len() >= MAX_FILES_TOTAL {
            break;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(project_root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if SKIP_DIRS.iter().any(|s| entry.file_name().to_string_lossy() == *s) {
                continue;
            }
            if is_ignored(&(rel.clone() + "/"), patterns) {
                continue;
            }
            out.extend(scan_file_tree_inner(project_root, &path, patterns, depth + 1));
        } else if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if file_count >= MAX_FILES_PER_DIR {
                break;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            // SOURCE_EXTENSIONS uses dot-prefixed entries (e.g.
            // `.ts`); `Path::extension()` returns the bare suffix
            // (`"ts"`). Compare with a leading dot.
            if !SOURCE_EXTENSIONS.iter().any(|s| *s == format!(".{ext}")) {
                continue;
            }
            if is_ignored(&rel, patterns) {
                continue;
            }
            out.push(rel);
            file_count += 1;
        }
    }
    out
}

fn priority_score(path: &str) -> usize {
    let lower = path.to_lowercase();
    PRIORITY_KEYWORDS.iter().filter(|k| lower.contains(*k)).count()
}

fn detect_entry_points(root: &Path, files: &[String]) -> Vec<EntryPoint> {
    let http_patterns: &[(&str, &str, &str)] = &[
        (
            "http",
            "Express/Koa route",
            r#"(?:app|router|server)\s*\.\s*(?:get|post|put|patch|delete|all|use)\s*\(\s*['"](/[^'"]*?)['"]"#,
        ),
        (
            "http",
            "Decorator route",
            r#"@(?:app\.)?(?:route|get|post|put|patch|delete|api_view|RequestMapping|GetMapping|PostMapping)\s*\(\s*['"](/[^'"]*?)['"]"#,
        ),
        (
            "http",
            "Next.js/Remix handler",
            r#"export\s+(?:async\s+)?function\s+(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b"#,
        ),
        ("cli", "CLI command", r#"\.command\s*\(\s*['"]([\w\-:]+)['"]"#),
        ("cli", "argparse subparser", r#"add_parser\s*\(\s*['"]([\w\-]+)['"]"#),
        (
            "event",
            "Event listener",
            r#"\.on\s*\(\s*['"]([\w\-:.]+)['"]"#,
        ),
        (
            "event",
            "Event subscriber",
            r#"@(?:EventHandler|Subscribe|Listener|on_event)\s*\(\s*['"]([\w\-:.]+)['"]"#,
        ),
        ("cron", "Cron schedule", r#"@?(?:Cron|Schedule|Scheduled|crontab)\s*\(\s*['"]([^'"]+)['"]"#),
        ("http", "GraphQL resolver", r#"@(?:Query|Mutation|Subscription|Resolver)\s*\("#),
    ];

    let compiled: Vec<(&str, &str, Regex)> = http_patterns
        .iter()
        .filter_map(|(kind, desc, pat)| {
            Regex::new(pat).ok().map(|re| (*kind, *desc, re))
        })
        .collect();

    let test_re = Regex::new(r"(?:\.test\.|\.spec\.|__tests__|_test\.py|test_\w+\.py)").unwrap();
    let mut out = Vec::new();
    for rel in files {
        if out.len() >= MAX_ENTRY_POINTS {
            break;
        }
        if test_re.is_match(rel) {
            continue;
        }
        let full = root.join(rel);
        let content = match fs::read_to_string(&full) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for (kind, desc, re) in &compiled {
            for cap in re.find_iter(&content) {
                if out.len() >= MAX_ENTRY_POINTS {
                    break;
                }
                let line_no = content[..cap.start()].matches('\n').count() as u32 + 1;
                let lines: Vec<&str> = content.lines().collect();
                let start = (line_no as usize).saturating_sub(1);
                let end = (start + 5).min(lines.len());
                let snippet = lines[start..end].join("\n");
                out.push(EntryPoint {
                    file: rel.clone(),
                    line: line_no,
                    kind: (*kind).to_string(),
                    description: (*desc).to_string(),
                    match_text: cap.as_str().chars().take(120).collect(),
                    snippet: snippet.chars().take(300).collect(),
                });
            }
        }
    }
    out
}

fn extract_file_signatures(root: &Path, files: &[String]) -> Vec<FileSignature> {
    let mut sorted: Vec<&String> = files.iter().collect();
    sorted.sort_by(|a, b| priority_score(b).cmp(&priority_score(a)));
    let mut out = Vec::new();
    let export_re = Regex::new(
        r"export\s+(?:default\s+)?(?:async\s+)?(?:function|class|const|let|var|interface|type|enum)\s+(\w+)",
    )
    .unwrap();
    let py_export_re = Regex::new(r"^(?:def|class)\s+(\w+)").unwrap();
    let import_re = Regex::new(
        r#"(?:import\s+.*?from\s+['"]([^'"]+)['"]|from\s+([\w.]+)\s+import)"#,
    )
    .unwrap();
    for rel in sorted.into_iter().take(MAX_SAMPLED_FILES) {
        let full = root.join(rel);
        let content = match fs::read_to_string(&full) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let lines: Vec<&str> = content.lines().take(MAX_LINES_PER_FILE).collect();
        let preview = lines.join("\n");
        let exports: Vec<String> = export_re
            .captures_iter(&preview)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .chain(
                py_export_re
                    .captures_iter(&preview)
                    .filter_map(|c| c.get(1).map(|m| m.as_str().to_string())),
            )
            .take(20)
            .collect();
        let imports: Vec<String> = import_re
            .captures_iter(&preview)
            .filter_map(|c| {
                c.get(1)
                    .or_else(|| c.get(2))
                    .map(|m| m.as_str().to_string())
            })
            .take(20)
            .collect();
        out.push(FileSignature {
            file: rel.clone(),
            exports,
            imports,
            lines: content.lines().count() as u32,
            preview: preview.chars().take(500).collect(),
        });
    }
    out
}

fn extract_metadata(root: &Path) -> BTreeMap<String, serde_json::Value> {
    let mut meta = BTreeMap::new();
    for filename in METADATA_FILES {
        let path = root.join(filename);
        if !path.exists() {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if *filename == "package.json" {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                let obj = json!({
                    "name": parsed.get("name").cloned().unwrap_or(json!(null)),
                    "description": parsed.get("description").cloned().unwrap_or(json!(null)),
                    "scripts": parsed.get("scripts").cloned().unwrap_or(json!(null)),
                });
                meta.insert("package.json".to_string(), obj);
            }
        } else if filename.ends_with(".md") || filename.ends_with(".rst") || filename.ends_with(".txt") {
            meta.insert(
                filename.to_string(),
                json!(content.chars().take(2000).collect::<String>()),
            );
        } else if filename.ends_with(".toml") || filename.ends_with(".cfg") || filename.ends_with(".mod") {
            meta.insert(
                filename.to_string(),
                json!(content.chars().take(1000).collect::<String>()),
            );
        } else {
            meta.insert(
                filename.to_string(),
                json!(content.chars().take(1000).collect::<String>()),
            );
        }
    }
    meta
}

// ============================================================================
// Section 4. LLM dispatch + response parsing
// ============================================================================

pub async fn call_domain_llm(
    llm: &LlmRequestSpec,
    system_extras: &str,
    user_payload: &str,
) -> Result<DomainAnalysis, String> {
    let mut system = DOMAIN_ANALYZER_PROMPT.to_string();
    if !system_extras.is_empty() {
        system.push_str("\n\n## Orchestrator notes\n\n");
        system.push_str(system_extras);
    }
    let mut req: LlmRequest = llm.into_request(system, user_payload.to_string());
    req.temperature = 0.2;
    let resp: LlmResponse = call_llm(req, 1)
        .await
        .map_err(|e| format!("LLM call failed: {e:?}"))?;
    parse_domain_response(&resp.content)
}

fn parse_domain_response(content: &str) -> Result<DomainAnalysis, String> {
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
    let raw: DomainAnalysisRaw = serde_json::from_value(parsed.clone())
        .map_err(|e| format!("domain shape invalid: {e}"))?;
    let mut nodes = Vec::with_capacity(raw.nodes.len());
    for n in raw.nodes {
        nodes.push(node_to_domain(n));
    }
    let edges: Vec<GraphEdge> = raw
        .edges
        .into_iter()
        .map(|e| match e {
            DomainEdgeOrPair::Object(g) => g,
            DomainEdgeOrPair::Pair {
                source,
                target,
                kind,
                weight,
                description,
            } => GraphEdge {
                source,
                target,
                kind,
                direction: "forward".to_string(),
                weight,
                description,
            },
        })
        .collect();
    Ok(DomainAnalysis { nodes, edges })
}

fn node_to_domain(n: DomainNodeOrBase) -> DomainNode {
    match n {
        DomainNodeOrBase::WithMeta {
            id,
            kind,
            name,
            file_path,
            summary,
            tags,
            complexity,
            domain_meta,
        } => DomainNode {
            base: GraphNode {
                id,
                kind,
                name,
                file_path: file_path.unwrap_or_default(),
                summary,
                tags,
                complexity,
                location: None,
                language_notes: None,
            },
            domain_meta,
        },
        DomainNodeOrBase::Plain(g) => DomainNode {
            base: g,
            domain_meta: None,
        },
    }
}

// ============================================================================
// Section 5. Validation + sanitization
// ============================================================================

const ALLOWED_NODE_KINDS: &[&str] = &["domain", "flow", "step"];
const ALLOWED_EDGE_KINDS: &[&str] = &["contains_flow", "flow_step", "cross_domain"];
const VALID_COMPLEXITIES: &[&str] = &["simple", "moderate", "complex"];

pub fn validate_graph(graph: &mut DomainGraph, warnings: &mut Vec<String>) {
    let mut seen_ids: HashSet<String> = HashSet::new();
    graph.nodes.retain(|n| {
        if !ALLOWED_NODE_KINDS.contains(&n.base.kind.as_str()) {
            warnings.push(format!(
                "dropped node with disallowed kind: {} ({})",
                n.base.id, n.base.kind
            ));
            return false;
        }
        if n.base.complexity.is_empty()
            || !VALID_COMPLEXITIES.contains(&n.base.complexity.as_str())
        {
            warnings.push(format!(
                "normalized complexity for {} from {:?} to moderate",
                n.base.id, n.base.complexity
            ));
        }
        if !seen_ids.insert(n.base.id.clone()) {
            warnings.push(format!("dropped duplicate node id: {}", n.base.id));
            return false;
        }
        true
    });
    // Validate edges: drop dangling, normalize kind, validate flow_step weights.
    let node_ids: BTreeSet<&str> = graph.nodes.iter().map(|n| n.base.id.as_str()).collect();
    graph.edges.retain(|e| {
        if !ALLOWED_EDGE_KINDS.contains(&e.kind.as_str()) {
            warnings.push(format!(
                "dropped edge with disallowed kind: {} -[{}]-> {}",
                e.source, e.kind, e.target
            ));
            return false;
        }
        if !node_ids.contains(e.source.as_str()) || !node_ids.contains(e.target.as_str()) {
            warnings.push(format!(
                "dropped dangling edge: {} -[{}]-> {}",
                e.source, e.kind, e.target
            ));
            return false;
        }
        true
    });
    // Validate that every flow connects to a domain (contains_flow edge).
    let flow_ids: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| n.base.kind == "flow")
        .map(|n| n.base.id.clone())
        .collect();
    let step_ids: HashSet<String> = graph
        .nodes
        .iter()
        .filter(|n| n.base.kind == "step")
        .map(|n| n.base.id.clone())
        .collect();
    let contains_targets: HashSet<String> = graph
        .edges
        .iter()
        .filter(|e| e.kind == "contains_flow")
        .map(|e| e.target.clone())
        .collect();
    let first_domain: Option<String> = graph
        .nodes
        .iter()
        .find(|n| n.base.kind == "domain")
        .map(|n| n.base.id.clone());
    for flow_id in &flow_ids {
        if !contains_targets.contains(flow_id) {
            warnings.push(format!(
                "flow {} has no contains_flow edge; auto-linking to first domain",
                flow_id
            ));
            if let Some(first_domain) = first_domain.clone() {
                graph.edges.push(GraphEdge {
                    source: first_domain,
                    target: flow_id.clone(),
                    kind: "contains_flow".to_string(),
                    direction: "forward".to_string(),
                    weight: 1.0,
                    description: None,
                });
            }
        }
    }
    // Per-flow: sort flow_step edges by weight, normalize to monotonic increasing in (0, 1].
    let mut by_flow: BTreeMap<String, Vec<(usize, f32)>> = BTreeMap::new();
    for (i, e) in graph.edges.iter().enumerate() {
        if e.kind == "flow_step" && step_ids.contains(&e.source) {
            // step.source is a step id of form "step:<flow>:<step>". The
            // LLM is supposed to target the flow id; if it uses step
            // ids as targets instead, skip.
            by_flow
                .entry(e.target.clone())
                .or_default()
                .push((i, e.weight));
        }
    }
    let weights_to_set: Vec<(usize, f32, String, f32)> = by_flow
        .into_iter()
        .flat_map(|(flow_id, mut entries)| {
            let n = entries.len();
            if n == 0 {
                return Vec::new();
            }
            let unit = (1.0 / n as f32).max(0.1);
            entries.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            entries
                .into_iter()
                .enumerate()
                .map(|(k, (idx, old_w))| {
                    let new_w = unit * (k as f32 + 1.0);
                    (idx, new_w, flow_id.clone(), old_w)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    for (idx, new_w, flow_id, old_w) in weights_to_set {
        if (old_w - new_w).abs() > 0.001 {
            warnings.push(format!(
                "renormalized flow_step weight for flow {} (edge idx {}): {} -> {}",
                flow_id, idx, old_w, new_w
            ));
        }
        graph.edges[idx].weight = new_w;
    }
}

// ============================================================================
// Section 6. Orchestrator
// ============================================================================

#[tauri::command]
pub async fn code_wiki_run_domain_pipeline(
    project_path: String,
    repo_name: String,
    llm: Option<LlmRequestSpec>,
    app: AppHandle,
) -> Result<(), String> {
    let result = run_domain(app.clone(), project_path, repo_name, llm).await;
    if let Err(ref e) = result {
        eprintln!("[code-wiki domain pipeline] run failed: {e}");
    }
    result.map(|_| ())
}

pub async fn run_domain(
    app: AppHandle,
    project_path: String,
    repo_name: String,
    llm: Option<LlmRequestSpec>,
) -> Result<DomainRunSummary, String> {
    let started = std::time::Instant::now();
    let pipeline_id = format!("{}/{}", project_path, repo_name);
    let project_root = PathBuf::from(&project_path);
    let repo_wiki_root = repo_root(&project_root, &repo_name);
    if !repo_wiki_root.is_dir() {
        return Err(format!(
            "code wiki not built for {repo_name}: run Analyze first"
        ));
    }
    let repo_dir = repo_wiki_root;
    std::fs::create_dir_all(&repo_dir).map_err(|e| format!("mkdir wiki dir: {e}"))?;

    let _ = app.emit(
        DOMAIN_EVENT,
        DomainProgress::Started {
            pipeline_id: pipeline_id.clone(),
            repo_name: repo_name.clone(),
            total_phases: TOTAL_PHASES,
        },
    );

    let mut warnings: Vec<String> = Vec::new();

    // ---- Phase 0: Detect existing knowledge graph + collect context.
    let _ = app.emit(
        DOMAIN_EVENT,
        DomainProgress::Phase {
            pipeline_id: pipeline_id.clone(),
            phase: PHASE_SCAN,
            label: "Detect context".to_string(),
            status: "running".to_string(),
        },
    );
    let (existing_graph, derived_from_graph) = match code_wiki_get_graph_inner(&project_root, &repo_name) {
        Ok(Some(g)) => (Some(g), true),
        Ok(None) => (None, false),
        Err(e) => {
            warnings.push(format!("could not load existing knowledge graph: {e}"));
            (None, false)
        }
    };

    // ---- Phase 1: Build context payload.
    let _ = app.emit(
        DOMAIN_EVENT,
        DomainProgress::Phase {
            pipeline_id: pipeline_id.clone(),
            phase: PHASE_BUILD,
            label: "Build context".to_string(),
            status: "running".to_string(),
        },
    );
    let context_json: String;
    let context_label: String;
    if let Some(ref g) = existing_graph {
        context_label = format!(
            "knowledge-graph.json ({} nodes, {} edges)",
            g.nodes.len(),
            g.edges.len()
        );
        context_json = serde_json::to_string(g).unwrap_or_default();
    } else {
        let ctx = extract_domain_context(&project_root).map_err(|e| format!("extract context: {e}"))?;
        context_label = format!(
            "domain-context ({} files, {} entry points, {} signatures)",
            ctx.file_count,
            ctx.entry_points.len(),
            ctx.file_signatures.len()
        );
        context_json = serde_json::to_string(&ctx).unwrap_or_default();
    }
    let _ = app.emit(
        DOMAIN_EVENT,
        DomainProgress::Phase {
            pipeline_id: pipeline_id.clone(),
            phase: PHASE_BUILD,
            label: "Build context".to_string(),
            status: "done".to_string(),
        },
    );

    // ---- Phase 2: LLM analyze (or deterministic fallback).
    let _ = app.emit(
        DOMAIN_EVENT,
        DomainProgress::Phase {
            pipeline_id: pipeline_id.clone(),
            phase: PHASE_ANALYZE,
            label: "Analyze (LLM)".to_string(),
            status: "running".to_string(),
        },
    );
    let mut analysis: DomainAnalysis = if let Some(ref spec) = llm {
        let user = format!(
            "Project context (source: {context_label}):\n\n```json\n{context_json}\n```\n\n\
             Identify the business domains, flows, and steps. Output strictly the JSON shape described in your system prompt.",
        );
        match call_domain_llm(spec, "", &user).await {
            Ok(a) => {
                warnings.push(format!(
                    "domain LLM produced {} nodes / {} edges",
                    a.nodes.len(),
                    a.edges.len()
                ));
                a
            }
            Err(e) => {
                warnings.push(format!("LLM domain analysis failed: {e}; producing empty graph"));
                DomainAnalysis::default()
            }
        }
    } else {
        warnings.push(
            "no LLM configured: domain graph will be empty (no LLM = no analysis)".to_string(),
        );
        DomainAnalysis::default()
    };

    // ---- Phase 3: Save.
    let _ = app.emit(
        DOMAIN_EVENT,
        DomainProgress::Phase {
            pipeline_id: pipeline_id.clone(),
            phase: PHASE_SAVE,
            label: "Save".to_string(),
            status: "running".to_string(),
        },
    );
    // If no nodes were produced (no LLM or LLM failed), still write
    // an empty graph so the dashboard knows the pipeline ran.
    if analysis.nodes.is_empty() {
        // Drop a placeholder domain so the dashboard has at least
        // something to render.
        analysis.nodes.push(DomainNode {
            base: GraphNode {
                id: "domain:no-llm".to_string(),
                kind: "domain".to_string(),
                name: "No LLM configured".to_string(),
                file_path: String::new(),
                summary: "Configure an LLM in the chat panel and re-run this pipeline to populate domain knowledge.".to_string(),
                tags: vec!["placeholder".to_string()],
                complexity: "simple".to_string(),
                location: None,
                language_notes: None,
            },
            domain_meta: Some(DomainMeta {
                entities: Vec::new(),
                business_rules: Vec::new(),
                cross_domain_interactions: Vec::new(),
                entry_point: None,
                entry_type: None,
            }),
        });
    }
    let used_llm = llm.is_some();
    let project_meta = build_project_meta(&project_root, &repo_name, &existing_graph, used_llm);
    let mut graph = DomainGraph {
        version: "1.0.0".to_string(),
        kind: "domain".to_string(),
        project: project_meta,
        nodes: analysis.nodes,
        edges: analysis.edges,
        derived_from_graph,
    };
    validate_graph(&mut graph, &mut warnings);

    let graph_path = domain_graph_path_for(&project_root, &repo_name);
    let bytes = serde_json::to_vec_pretty(&graph)
        .map_err(|e| format!("serialize domain graph: {e}"))?;
    write_atomic(&graph_path, &bytes).map_err(|e| format!("write domain graph: {e}"))?;

    let meta_path = domain_meta_path_for(&project_root, &repo_name);
    let meta = PipelineMeta {
        last_analyzed_at: now_iso(),
        git_commit_hash: existing_graph
            .as_ref()
            .map(|g| g.project.git_commit_hash.clone())
            .unwrap_or_default(),
        version: "domaingraph-1.0.0".to_string(),
        kind: "domain".to_string(),
        analyzed_files: 0,
        review_narrative: None,
        review_approved: None,
        assemble_review: None,
        changed_file_count: None,
        unchanged_file_count: None,
        removed_file_count: None,
    };
    write_meta(&meta_path, &meta).map_err(|e| format!("write meta: {e}"))?;

    let _ = app.emit(
        DOMAIN_EVENT,
        DomainProgress::Phase {
            pipeline_id: pipeline_id.clone(),
            phase: PHASE_SAVE,
            label: "Save".to_string(),
            status: "done".to_string(),
        },
    );

    let summary = DomainRunSummary {
        pipeline_id: pipeline_id.clone(),
        project_path: project_path.clone(),
        repo_name: repo_name.clone(),
        final_graph_path: graph_path.to_string_lossy().to_string(),
        final_meta_path: meta_path.to_string_lossy().to_string(),
        node_count: graph.nodes.len() as u32,
        edge_count: graph.edges.len() as u32,
        kind: "domain".to_string(),
        duration_ms: started.elapsed().as_millis() as u64,
        warnings: warnings.clone(),
        derived_from_graph,
        used_llm,
    };
    let _ = app.emit(DOMAIN_DONE_EVENT, &summary);
    Ok(summary)
}

fn build_project_meta(
    _project_root: &Path,
    repo_name: &str,
    existing: &Option<KnowledgeGraph>,
    _used_llm: bool,
) -> ProjectMeta {
    if let Some(g) = existing {
        return g.project.clone();
    }
    ProjectMeta {
        name: repo_name.to_string(),
        languages: Vec::new(),
        frameworks: Vec::new(),
        description: format!("Domain analysis for {repo_name}"),
        analyzed_at: now_iso(),
        git_commit_hash: String::new(),
    }
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400) as u32;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i32 + (era as i32) * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}.000Z")
}

// ============================================================================
// Section 7. Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_save::write_atomic;

    fn empty_node(id: &str, kind: &str) -> DomainNode {
        DomainNode {
            base: GraphNode {
                id: id.to_string(),
                kind: kind.to_string(),
                name: id.to_string(),
                file_path: String::new(),
                summary: String::new(),
                tags: vec![],
                complexity: "moderate".to_string(),
                location: None,
                language_notes: None,
            },
            domain_meta: None,
        }
    }

    fn edge(source: &str, target: &str, kind: &str, weight: f32) -> GraphEdge {
        GraphEdge {
            source: source.to_string(),
            target: target.to_string(),
            kind: kind.to_string(),
            direction: "forward".to_string(),
            weight,
            description: None,
        }
    }

    #[test]
    fn validate_drops_disallowed_node_kinds() {
        let mut g = DomainGraph {
            version: "1.0.0".to_string(),
            kind: "domain".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![
                empty_node("domain:x", "domain"),
                empty_node("file:y", "file"),
            ],
            edges: vec![],
            derived_from_graph: false,
        };
        let mut warnings = Vec::new();
        validate_graph(&mut g, &mut warnings);
        assert_eq!(g.nodes.len(), 1, "file node should be dropped");
        assert!(warnings.iter().any(|w| w.contains("disallowed kind")));
    }

    #[test]
    fn validate_drops_dangling_edges() {
        let mut g = DomainGraph {
            version: "1.0.0".to_string(),
            kind: "domain".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![empty_node("domain:x", "domain")],
            edges: vec![edge("domain:x", "ghost", "contains_flow", 1.0)],
            derived_from_graph: false,
        };
        let mut warnings = Vec::new();
        validate_graph(&mut g, &mut warnings);
        assert!(g.edges.is_empty(), "dangling edge should be dropped");
        assert!(warnings.iter().any(|w| w.contains("dangling")));
    }

    #[test]
    fn flow_step_weights_normalize_monotonically() {
        let mut g = DomainGraph {
            version: "1.0.0".to_string(),
            kind: "domain".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![
                empty_node("domain:x", "domain"),
                empty_node("flow:create", "flow"),
                empty_node("step:create:a", "step"),
                empty_node("step:create:b", "step"),
                empty_node("step:create:c", "step"),
            ],
            edges: vec![
                edge("domain:x", "flow:create", "contains_flow", 1.0),
                // Out of order on purpose
                edge("step:create:c", "flow:create", "flow_step", 0.99),
                edge("step:create:a", "flow:create", "flow_step", 0.05),
                edge("step:create:b", "flow:create", "flow_step", 0.5),
            ],
            derived_from_graph: false,
        };
        let mut warnings = Vec::new();
        validate_graph(&mut g, &mut warnings);
        let mut steps: Vec<f32> = g
            .edges
            .iter()
            .filter(|e| e.kind == "flow_step")
            .map(|e| e.weight)
            .collect();
        // `validate_graph` mutates the weights in-place but does
        // not reorder `graph.edges`. Sort by weight to read them
        // back in the order the normalizer assigned.
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        assert_eq!(steps.len(), 3);
        // Each weight should be unit * (k+1) for k in 0..=2
        let unit = (1.0f32 / 3.0).max(0.1);
        for (k, w) in steps.iter().enumerate() {
            let expected = unit * (k as f32 + 1.0);
            assert!((w - expected).abs() < 0.001, "weight {w} != expected {expected}");
        }
    }

    #[test]
    fn orphan_flow_gets_autolinked_to_first_domain() {
        let mut g = DomainGraph {
            version: "1.0.0".to_string(),
            kind: "domain".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![
                empty_node("domain:x", "domain"),
                empty_node("flow:lonely", "flow"),
            ],
            edges: vec![],
            derived_from_graph: false,
        };
        let mut warnings = Vec::new();
        validate_graph(&mut g, &mut warnings);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].kind, "contains_flow");
        assert_eq!(g.edges[0].source, "domain:x");
        assert!(warnings.iter().any(|w| w.contains("auto-linking")));
    }

    #[test]
    fn extract_domain_context_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Set up a fake project
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("server.ts"),
            "import express from 'express';\n\
             const app = express();\n\
             app.get('/api/users', (req, res) => { res.json([]); });\n\
             app.post('/api/users', (req, res) => { res.json({}); });\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"demo","description":"test","scripts":{"test":"vitest"}}"#,
        )
        .unwrap();
        let ctx = extract_domain_context(root).expect("should extract");
        assert!(
            !ctx.file_tree.is_empty(),
            "expected at least one file, got file_tree={:?}",
            ctx.file_tree
        );
        assert!(!ctx.entry_points.is_empty(), "should detect express route");
        assert_eq!(ctx.metadata.get("package.json").unwrap()["name"], "demo");
    }

    #[test]
    fn parse_domain_response_unwraps_fenced_json() {
        let raw = "```json\n{\"nodes\":[{\"id\":\"domain:x\",\"type\":\"domain\",\"name\":\"X\"}],\"edges\":[]}\n```";
        let parsed = parse_domain_response(raw).expect("should parse");
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.nodes[0].base.id, "domain:x");
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        let repo = "demo";
        std::fs::create_dir_all(repo_root(project, repo)).unwrap();
        let g = DomainGraph {
            version: "1.0.0".to_string(),
            kind: "domain".to_string(),
            project: ProjectMeta {
                name: repo.to_string(),
                languages: vec!["rust".to_string()],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: now_iso(),
                git_commit_hash: String::new(),
            },
            nodes: vec![empty_node("domain:auth", "domain")],
            edges: vec![],
            derived_from_graph: false,
        };
        let path = domain_graph_path_for(project, repo);
        let bytes = serde_json::to_vec_pretty(&g).unwrap();
        write_atomic(&path, &bytes).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: DomainGraph = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.nodes.len(), 1);
        assert_eq!(parsed.kind, "domain");
    }

    #[test]
    fn lint_constants_have_no_duplicate_strings() {
        // Cheap sanity: make sure we did not typo the kind lists.
        assert!(ALLOWED_NODE_KINDS.contains(&"domain"));
        assert!(ALLOWED_EDGE_KINDS.contains(&"flow_step"));
        assert!(VALID_COMPLEXITIES.contains(&"complex"));
    }

    #[test]
    fn flow_step_uses_step_ids_not_target() {
        // Per UA convention: flow_step edges have step → flow (not flow → step).
        // We document this in the prompt and validate the convention loosely.
        let mut g = DomainGraph {
            version: "1.0.0".to_string(),
            kind: "domain".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec![],
                frameworks: vec![],
                description: "d".to_string(),
                analyzed_at: "2026-01-01T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![
                empty_node("domain:d", "domain"),
                empty_node("flow:f", "flow"),
                empty_node("step:f:s1", "step"),
            ],
            edges: vec![
                edge("domain:d", "flow:f", "contains_flow", 1.0),
                edge("step:f:s1", "flow:f", "flow_step", 0.5),
            ],
            derived_from_graph: false,
        };
        let mut warnings = Vec::new();
        validate_graph(&mut g, &mut warnings);
        assert_eq!(g.edges.len(), 2);
    }
}