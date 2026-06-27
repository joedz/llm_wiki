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

#[cfg(test)]
#[path = "code_wiki_tests.rs"]
mod tests;
