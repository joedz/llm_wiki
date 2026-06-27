use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const WIKI_CODE_WIKI_DIR: &str = "wiki/code_wiki";
pub const CODEGRAPH_DIR_NAME: &str = ".codegraph";
pub const INDEX_FILE: &str = "index.json";
pub const GRAPH_FILE: &str = "graph.json";
pub const META_FILE: &str = "meta.json";

pub fn repo_root(project_path: &Path, repo_name: &str) -> PathBuf {
    project_path.join(WIKI_CODE_WIKI_DIR).join(repo_name)
}

pub fn codegraph_dir_for(project_path: &Path, repo_name: &str) -> PathBuf {
    repo_root(project_path, repo_name).join(CODEGRAPH_DIR_NAME)
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
    normalized.ends_with("/graph.json")
        || normalized.ends_with("/meta.json")
        || normalized == "wiki/code_wiki/index.json"
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RepoSummary {
    pub name: String,
    pub path: String,
    pub graph_path: String,
    pub languages: Vec<String>,
    pub file_count: u32,
    pub symbol_count: u32,
    pub description: Option<String>,
    pub last_analyzed_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodeWikiIndex {
    pub version: String,
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

fn detect_codegraph() -> CodeWikiInstallStatus {
    match std::process::Command::new("codegraph").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
            CodeWikiInstallStatus {
                installed: true,
                version: Some(version.clone()),
                path: which_codegraph(),
                message: format!("codegraph {} available", version),
            }
        }
        _ => CodeWikiInstallStatus {
            installed: false,
            version: None,
            path: which_codegraph(),
            message: "codegraph CLI not found on PATH".to_string(),
        },
    }
}

fn which_codegraph() -> Option<String> {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    std::process::Command::new(cmd)
        .arg("codegraph")
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
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
}

pub fn plan_index_invocation(project_path: &Path, repo_name: &str) -> IndexInvocationPlan {
    let repo_root = project_path.join("raw").join("code").join(repo_name);
    let codegraph_dir = codegraph_dir_for(project_path, repo_name);
    IndexInvocationPlan { repo_root, codegraph_dir }
}

pub fn run_indexer_inner(project_path: &Path, repo_name: &str) -> Result<(), String> {
    let plan = plan_index_invocation(project_path, repo_name);
    fs::create_dir_all(&plan.codegraph_dir)
        .map_err(|e| format!("mkdir codegraph dir: {}", e))?;
    let init_status = std::process::Command::new("codegraph")
        .arg("init")
        .arg(&plan.repo_root)
        .status()
        .map_err(|e| format!("spawn codegraph init: {}", e))?;
    if !init_status.success() {
        return Err(format!(
            "codegraph init exited with {:?}",
            init_status.code()
        ));
    }
    let index_status = std::process::Command::new("codegraph")
        .arg("index")
        .arg(&plan.repo_root)
        .status()
        .map_err(|e| format!("spawn codegraph index: {}", e))?;
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
    let status = std::process::Command::new("codegraph")
        .arg("sync")
        .arg(&plan.repo_root)
        .status()
        .map_err(|e| format!("spawn codegraph sync: {}", e))?;
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodegraphContextNode {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub complexity: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub location: Option<NodeLocation>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeLocation {
    pub start_line: u32,
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
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub git_commit_hash: Option<String>,
    pub nodes: Vec<CodegraphContextNode>,
    pub edges: Vec<CodegraphContextEdge>,
}

pub fn run_get_graph_payload_inner(
    project_path: &Path,
    repo_name: &str,
) -> Result<CodegraphContextPayload, String> {
    let plan = plan_index_invocation(project_path, repo_name);
    if !plan.repo_root.exists() {
        return Err(format!("repo path {:?} does not exist", plan.repo_root));
    }
    let output = std::process::Command::new("codegraph")
        .arg("context")
        .arg("--format")
        .arg("json")
        .arg(&plan.repo_root)
        .output()
        .map_err(|e| format!("spawn codegraph context: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "codegraph context exited with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("parse codegraph context: {}", e))
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
