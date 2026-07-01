use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

pub const WIKI_CODE_WIKI_DIR: &str = "wiki/code_wiki";
pub const CODEGRAPH_DIR_NAME: &str = ".codegraph";
pub const INDEX_FILE: &str = "index.json";
pub const GRAPH_FILE: &str = "knowledge-graph.json";
pub const META_FILE: &str = "meta.json";

pub fn repo_root(project_path: &Path, repo_name: &str) -> PathBuf {
    project_path.join(WIKI_CODE_WIKI_DIR).join(repo_name)
}

/// The source code directory for a repo — where actual source files live.
/// This is distinct from repo_root() which points to the wiki output dir.
pub fn source_dir_for(project_path: &Path, repo_name: &str) -> PathBuf {
    project_path.join("raw").join("code").join(repo_name)
}

/// codegraph's own DB lives next to the source files it indexed, in
/// `raw/code/<repo>/.codegraph/`. We don't try to relocate it: the tool
/// requires this layout (no `--db-path` flag in 0.9.x), and the hidden
/// `.codegraph/` keeps the user's source tree clean in practice.
pub fn codegraph_dir_for(project_path: &Path, repo_name: &str) -> PathBuf {
    project_path
        .join("raw")
        .join("code")
        .join(repo_name)
        .join(CODEGRAPH_DIR_NAME)
}

pub fn graph_path_for(project_path: &Path, repo_name: &str) -> PathBuf {
    repo_root(project_path, repo_name).join(GRAPH_FILE)
}

pub fn meta_path_for(project_path: &Path, repo_name: &str) -> PathBuf {
    repo_root(project_path, repo_name).join(META_FILE)
}

pub fn index_path_for(project_path: &Path) -> PathBuf {
    project_path.join(WIKI_CODE_WIKI_DIR).join(INDEX_FILE)
}

pub fn is_code_wiki_public_path(rel: &str) -> bool {
    let normalized = rel.replace('\\', "/").to_lowercase();
    if !normalized.starts_with("wiki/code_wiki/") {
        return false;
    }
    if normalized.contains("/.codegraph/") {
        return false;
    }
    normalized.ends_with("/knowledge-graph.json")
        || normalized.ends_with("/meta.json")
        || normalized == "wiki/code_wiki/index.json"
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RepoSummary {
    pub name: String,
    pub path: String,
    #[serde(alias = "graphPath")]
    pub graph_path: String,
    pub languages: Vec<String>,
    #[serde(alias = "fileCount")]
    pub file_count: u32,
    #[serde(alias = "symbolCount")]
    pub symbol_count: u32,
    pub description: Option<String>,
    #[serde(alias = "lastAnalyzedAt")]
    pub last_analyzed_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodeWikiIndex {
    pub version: String,
    #[serde(alias = "generatedAt")]
    pub generated_at: String,
    pub repos: Vec<RepoSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CodeWikiInstallStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub message: String,
}

/// Resolve the absolute path to the `codegraph` executable on this machine.
/// Uses the `which` crate so nvm4w / `.npm-global` installs (which the Rust
/// `Command::new("codegraph")` PATH lookup often misses on Windows) work too.
fn resolve_codegraph_bin() -> Option<PathBuf> {
    which::which("codegraph").ok()
}

fn detect_codegraph() -> CodeWikiInstallStatus {
    match resolve_codegraph_bin() {
        Some(bin) => match Command::new(&bin).arg("--version").output() {
            Ok(out) if out.status.success() => {
                let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
                CodeWikiInstallStatus {
                    installed: true,
                    version: Some(version.clone()),
                    path: Some(bin.to_string_lossy().to_string()),
                    message: format!("codegraph {} available", version),
                }
            }
            Ok(_) => CodeWikiInstallStatus {
                installed: false,
                version: None,
                path: Some(bin.to_string_lossy().to_string()),
                message: "codegraph binary found but --version failed".to_string(),
            },
            Err(err) => CodeWikiInstallStatus {
                installed: false,
                version: None,
                path: Some(bin.to_string_lossy().to_string()),
                message: format!("codegraph binary at {:?} not executable: {}", bin, err),
            },
        },
        None => CodeWikiInstallStatus {
            installed: false,
            version: None,
            path: None,
            message: "codegraph CLI not found on PATH".to_string(),
        },
    }
}

#[tauri::command]
pub async fn code_wiki_install_check() -> Result<CodeWikiInstallStatus, String> {
    Ok(detect_codegraph())
}

pub fn list_repo_names(project_path: &Path) -> Result<Vec<String>, String> {
    let code_root = project_path.join("raw").join("code");
    let entries = match fs::read_dir(&code_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read_dir({:?}): {}", code_root, e)),
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.') && n != "node_modules")
        .collect();
    names.sort();
    Ok(names)
}

pub fn read_or_empty_index(project_path: &Path) -> Result<CodeWikiIndex, String> {
    let path = index_path_for(project_path);
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| format!("parse index: {}", e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CodeWikiIndex {
            version: "1.0.0".to_string(),
            generated_at: String::new(),
            repos: Vec::new(),
        }),
        Err(e) => Err(format!("read index: {}", e)),
    }
}

#[tauri::command]
pub async fn code_wiki_list_repos(project_path: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || list_repo_names(Path::new(&project_path)))
        .await
        .map_err(|e| format!("join error: {}", e))?
}

#[tauri::command]
pub async fn code_wiki_get_index(project_path: String) -> Result<CodeWikiIndex, String> {
    tauri::async_runtime::spawn_blocking(move || read_or_empty_index(Path::new(&project_path)))
        .await
        .map_err(|e| format!("join error: {}", e))?
}

#[tauri::command]
pub async fn code_wiki_get_graph(
    project_path: String,
    repo_name: String,
) -> Result<Option<serde_json::Value>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = graph_path_for(Path::new(&project_path), &repo_name);
        match fs::read_to_string(&path) {
            Ok(raw) => {
                serde_json::from_str(&raw).map(Some).map_err(|e| format!("parse graph: {}", e))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("read graph: {}", e)),
        }
    })
    .await
    .map_err(|e| format!("join error: {}", e))?
}

#[derive(Debug, PartialEq, Eq)]
pub struct IndexInvocationPlan {
    pub repo_root: PathBuf,
    pub codegraph_dir: PathBuf,
    pub codegraph_db: PathBuf,
}

pub fn plan_index_invocation(project_path: &Path, repo_name: &str) -> IndexInvocationPlan {
    let repo_root = project_path.join("raw").join("code").join(repo_name);
    let codegraph_dir = codegraph_dir_for(project_path, repo_name);
    let codegraph_db = codegraph_dir.join("codegraph.db");
    IndexInvocationPlan {
        repo_root,
        codegraph_dir,
        codegraph_db,
    }
}

/// Run a `codegraph` subcommand. Resolves the binary via `which` (so nvm4w-style
/// installs work) and surfaces a clean error when the CLI is missing.
fn run_codegraph(args: &[&str], repo_root: &Path) -> Result<std::process::ExitStatus, String> {
    let bin = resolve_codegraph_bin()
        .ok_or_else(|| "codegraph CLI not found on PATH (try `npm i -g @colbymchenry/codegraph`)".to_string())?;
    let mut cmd = Command::new(&bin);
    cmd.args(args);
    cmd.arg(repo_root);
    cmd.status()
        .map_err(|e| format!("spawn {:?}: {}", bin, e))
}

pub fn run_indexer_inner(project_path: &Path, repo_name: &str) -> Result<(), String> {
    let plan = plan_index_invocation(project_path, repo_name);
    fs::create_dir_all(&plan.codegraph_dir)
        .map_err(|e| format!("mkdir codegraph dir: {}", e))?;
    let init_status = run_codegraph(&["init"], &plan.repo_root)?;
    if !init_status.success() {
        return Err(format!(
            "codegraph init exited with {:?}",
            init_status.code()
        ));
    }
    let index_status = run_codegraph(&["index"], &plan.repo_root)?;
    if !index_status.success() {
        return Err(format!(
            "codegraph index exited with {:?}",
            index_status.code()
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn code_wiki_run_indexer(project_path: String, repo_name: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_indexer_inner(Path::new(&project_path), &repo_name)
    })
    .await
    .map_err(|e| format!("join error: {}", e))?
}

pub fn run_sync_inner(project_path: &Path, repo_name: &str) -> Result<(), String> {
    let plan = plan_index_invocation(project_path, repo_name);
    if !plan.repo_root.exists() {
        return Err(format!("repo path {:?} no longer exists", plan.repo_root));
    }
    let status = run_codegraph(&["sync"], &plan.repo_root)?;
    if !status.success() {
        return Err(format!("codegraph sync exited with {:?}", status.code()));
    }
    Ok(())
}

#[tauri::command]
pub async fn code_wiki_run_sync(project_path: String, repo_name: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || run_sync_inner(Path::new(&project_path), &repo_name))
        .await
        .map_err(|e| format!("join error: {}", e))?
}

pub fn affected_repos(changes: &[String]) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    for change in changes {
        let parts: Vec<&str> = change.split('/').collect();
        if parts.len() >= 3 && parts[0] == "raw" && parts[1] == "code" {
            set.insert(parts[2].to_string());
        }
    }
    set.into_iter().collect()
}

// --- codegraph DB → CodegraphContextPayload ---------------------------------
//
// codegraph 0.9.x doesn't expose a "context" subcommand, so we read the SQLite
// store at `.codegraph/codegraph.db` directly. The schema we care about is the
// `nodes` and `edges` tables; everything else is metadata we re-derive (language
// tally, project path, etc.) from the same DB.

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodegraphContextNode {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(default, rename = "qualifiedName")]
    pub qualified_name: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub docstring: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub location: Option<NodeLocation>,
    #[serde(default, rename = "isExported")]
    pub is_exported: Option<bool>,
    #[serde(default, rename = "isAsync")]
    pub is_async: Option<bool>,
    #[serde(default)]
    pub decorators: Vec<String>,
    #[serde(default)]
    pub visibility: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeLocation {
    #[serde(rename = "startLine")]
    pub start_line: u32,
    #[serde(rename = "endLine")]
    pub end_line: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodegraphContextEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub weight: Option<f32>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodegraphContextPayload {
    #[serde(default, rename = "projectPath")]
    pub project_path: Option<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default, rename = "gitCommitHash")]
    pub git_commit_hash: Option<String>,
    pub nodes: Vec<CodegraphContextNode>,
    pub edges: Vec<CodegraphContextEdge>,
}

fn read_db_payload(db_path: &Path) -> Result<CodegraphContextPayload, String> {
    if !db_path.exists() {
        return Err(format!("codegraph DB not found at {:?}", db_path));
    }
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open codegraph DB: {}", e))?;

    let project_path: Option<String> = conn
        .query_row(
            "SELECT value FROM project_metadata WHERE key = 'projectPath'",
            [],
            |row| row.get(0),
        )
        .ok();

    let mut lang_stmt = conn
        .prepare("SELECT DISTINCT language FROM nodes WHERE language <> '' ORDER BY language")
        .map_err(|e| format!("prepare languages: {}", e))?;
    let languages: Vec<String> = lang_stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("query languages: {}", e))?
        .filter_map(|r| r.ok())
        .collect();

    let git_commit_hash: Option<String> = conn
        .query_row(
            "SELECT value FROM project_metadata WHERE key IN ('gitCommitHash','git_commit_hash')",
            [],
            |row| row.get(0),
        )
        .ok();

    let mut node_stmt = conn
        .prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    docstring, signature, visibility, is_exported, is_async,
                    decorators, start_line, end_line
             FROM nodes",
        )
        .map_err(|e| format!("prepare nodes: {}", e))?;
    let nodes: Vec<CodegraphContextNode> = node_stmt
        .query_map([], row_to_node)
        .map_err(|e| format!("query nodes: {}", e))?
        .filter_map(|r| r.ok())
        .collect();

    let mut edge_stmt = conn
        .prepare("SELECT source, target, kind, metadata FROM edges")
        .map_err(|e| format!("prepare edges: {}", e))?;
    let edges: Vec<CodegraphContextEdge> = edge_stmt
        .query_map([], row_to_edge)
        .map_err(|e| format!("query edges: {}", e))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(CodegraphContextPayload {
        project_path,
        languages,
        git_commit_hash,
        nodes,
        edges,
    })
}

fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodegraphContextNode> {
    let decorators_json: Option<String> = row.get(11)?;
    let decorators: Vec<String> = decorators_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let start_line: i64 = row.get(12)?;
    let end_line: i64 = row.get(13)?;
    Ok(CodegraphContextNode {
        id: row.get(0)?,
        kind: row.get(1)?,
        name: row.get(2)?,
        qualified_name: row.get(3)?,
        file_path: row.get(4)?,
        language: row.get(5)?,
        docstring: row.get(6)?,
        signature: row.get(7)?,
        visibility: row.get(8)?,
        is_exported: row.get::<_, Option<i64>>(9)?.map(|v| v != 0),
        is_async: row.get::<_, Option<i64>>(10)?.map(|v| v != 0),
        decorators,
        summary: row.get(6)?,    // alias docstring → summary for the exporter
        location: Some(NodeLocation {
            start_line: start_line.max(0) as u32,
            end_line: end_line.max(0) as u32,
        }),
        tags: Vec::new(),
    })
}

fn row_to_edge(row: &rusqlite::Row<'_>) -> rusqlite::Result<CodegraphContextEdge> {
    let metadata_json: Option<String> = row.get(3)?;
    let metadata = metadata_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    Ok(CodegraphContextEdge {
        source: row.get(0)?,
        target: row.get(1)?,
        kind: row.get(2)?,
        weight: None,
        metadata,
    })
}

pub fn run_get_graph_payload_inner(
    project_path: &Path,
    repo_name: &str,
) -> Result<CodegraphContextPayload, String> {
    let plan = plan_index_invocation(project_path, repo_name);
    if !plan.repo_root.exists() {
        return Err(format!("repo path {:?} does not exist", plan.repo_root));
    }
    read_db_payload(&plan.codegraph_db)
}

#[tauri::command]
pub async fn code_wiki_get_graph_payload(
    project_path: String,
    repo_name: String,
) -> Result<CodegraphContextPayload, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_get_graph_payload_inner(Path::new(&project_path), &repo_name)
    })
    .await
    .map_err(|e| format!("join error: {}", e))?
}

#[cfg(test)]
#[path = "code_wiki_tests.rs"]
mod tests;
