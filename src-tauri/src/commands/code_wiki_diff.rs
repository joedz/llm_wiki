// Diff overlay — mirrors UA's `understand-diff` skill + the
// `diff-overlay.json` file the dashboard already knows how to
// render. We compute:
//
//   - `changedFiles`: paths returned by `git status --porcelain`
//     (uncommitted + staged) or `git diff <base>..HEAD --name-only`
//     when a base ref is provided.
//   - `changedNodeIds`: graph nodes whose `filePath` matches one
//     of `changedFiles`. Includes both file-level and symbol-level
//     (function/class) nodes so the dashboard can highlight
//     "this exact function changed" granularity.
//   - `affectedNodeIds`: nodes one hop away from any changed
//     node. We walk both directions of every edge — upstream
//     callers and downstream callees — so the analyst sees the
//     full blast radius of the change.
//
// The overlay is written to `<repo>/.understand/diff-overlay.json`
// and also returned to the caller. The dashboard polls the file
// on load and on file-watch events.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::commands::code_wiki_pipeline::KnowledgeGraph;
use crate::commands::code_wiki_save::write_atomic;

const DIFF_OVERLAY_FILE: &str = "diff-overlay.json";
const DIFF_OVERLAY_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffOverlay {
    pub version: String,
    #[serde(rename = "baseBranch")]
    pub base_branch: String,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    #[serde(rename = "changedFiles")]
    pub changed_files: Vec<String>,
    #[serde(rename = "changedNodeIds")]
    pub changed_node_ids: Vec<String>,
    #[serde(rename = "affectedNodeIds")]
    pub affected_node_ids: Vec<String>,
    pub warnings: Vec<String>,
}

/// Run `git` in the given directory. Returns `None` if the
/// directory is not a git repo, the command fails, or the
/// binary is missing.
fn run_git(repo_root: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// List changed files. If `base` is Some, diff against that ref;
/// otherwise use `git status --porcelain` to capture uncommitted
/// changes too.
fn list_changed_files(repo_root: &Path, base: Option<&str>) -> (Vec<String>, Vec<String>) {
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut warnings: Vec<String> = Vec::new();

    if let Some(b) = base {
        let out = match run_git(repo_root, &["diff", "--name-only", b, "HEAD"]) {
            Some(s) => s,
            None => {
                warnings.push(format!("git diff {}..HEAD failed", b));
                return (Vec::new(), warnings);
            }
        };
        for line in out.lines() {
            let t = line.trim();
            if !t.is_empty() {
                files.insert(normalize_path(t));
            }
        }
    } else {
        // Uncommitted + staged. Porcelain v1 is widely supported.
        let out = match run_git(
            repo_root,
            &["status", "--porcelain", "--untracked-files=all"],
        ) {
            Some(s) => s,
            None => {
                warnings.push("git status failed (not a git repo or git missing)".to_string());
                return (Vec::new(), warnings);
            }
        };
        for line in out.lines() {
            // Porcelain v1 format: "XY <path>" where XY is a
            // two-letter status. We skip rename lines ("R ",
            // "RM") which print "old -> new" on a second line.
            if line.len() < 3 {
                continue;
            }
            let status = &line[..2];
            if status.starts_with('R') || status.starts_with('C') {
                // Use the new path (after the -> arrow) if
                // present, else the first path.
                if let Some(idx) = line.find("-> ") {
                    files.insert(normalize_path(&line[idx + 3..]));
                } else {
                    files.insert(normalize_path(&line[3..]));
                }
            } else {
                files.insert(normalize_path(&line[3..]));
            }
        }
    }
    (files.into_iter().collect(), warnings)
}

fn normalize_path(p: &str) -> String {
    let t = p.trim().trim_matches('"');
    t.strip_prefix("./").unwrap_or(t).replace('\\', "/")
}

/// Match the changed files against the graph and return
/// `(changed_node_ids, affected_node_ids)`. Affected nodes are
/// the 1-hop neighbourhood of changed nodes (both directions).
pub fn compute_diff(graph: &KnowledgeGraph, changed_files: &[String]) -> (Vec<String>, Vec<String>) {
    // Pre-build a set of normalised file paths for fast lookup.
    let file_index: BTreeSet<String> = changed_files.iter().map(|s| normalize_path(s)).collect();

    // 1. Match file paths.
    let mut changed_node_ids: BTreeSet<String> = BTreeSet::new();
    for node in &graph.nodes {
        if node.file_path.is_empty() {
            continue;
        }
        let normalised = normalize_path(&node.file_path);
        if file_index.contains(&normalised) {
            changed_node_ids.insert(node.id.clone());
        }
    }

    // 2. 1-hop affected: walk all edges; if either endpoint is
    //    in changed_node_ids, the other end is affected.
    let mut affected: BTreeSet<String> = BTreeSet::new();
    for edge in &graph.edges {
        let src_changed = changed_node_ids.contains(&edge.source);
        let tgt_changed = changed_node_ids.contains(&edge.target);
        if src_changed && !tgt_changed {
            affected.insert(edge.target.clone());
        } else if tgt_changed && !src_changed {
            affected.insert(edge.source.clone());
        } else if src_changed && tgt_changed {
            // Both endpoints changed — they're already in
            // changed_node_ids. Skip (don't double-count).
        }
    }

    (changed_node_ids.into_iter().collect(), affected.into_iter().collect())
}

/// Compute the diff overlay for a repo. Returns the overlay
/// (or None if the repo is not a git repo and we can't
/// determine changes). Writes the overlay to
/// `<understand>/diff-overlay.json` so the dashboard server
/// can serve it as `/diff-overlay.json?token=...`.
pub fn refresh_diff_overlay_inner(
    project_root: &Path,
    repo_name: &str,
    base: Option<&str>,
) -> Result<Option<DiffOverlay>, String> {
    let repo_dir = crate::commands::code_wiki::repo_root(project_root, repo_name);
    if !repo_dir.is_dir() {
        return Err(format!("repo not found: {}", repo_dir.display()));
    }
    let (changed_files, mut warnings) = list_changed_files(&repo_dir, base);
    let graph_path = crate::commands::code_wiki::graph_path_for(project_root, repo_name);
    let graph: KnowledgeGraph = if graph_path.exists() {
        let raw = fs::read_to_string(&graph_path)
            .map_err(|e| format!("read knowledge-graph.json: {e}"))?;
        serde_json::from_str(&raw)
            .map_err(|e| format!("parse knowledge-graph.json: {e}"))?
    } else {
        warnings.push("knowledge-graph.json missing; affected set is empty".to_string());
        KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: crate::commands::code_wiki_pipeline::ProjectMeta {
                name: repo_name.to_string(),
                languages: Vec::new(),
                frameworks: Vec::new(),
                description: String::new(),
                analyzed_at: String::new(),
                git_commit_hash: String::new(),
            },
            nodes: Vec::new(),
            edges: Vec::new(),
            layers: Vec::new(),
            tour: Vec::new(),
        }
    };
    let (changed_node_ids, affected_node_ids) = compute_diff(&graph, &changed_files);
    if changed_files.is_empty() && changed_node_ids.is_empty() {
        warnings.push("no changes detected in the working tree".to_string());
    }
    let overlay = DiffOverlay {
        version: DIFF_OVERLAY_VERSION.to_string(),
        base_branch: base.unwrap_or("HEAD").to_string(),
        generated_at: now_iso(),
        changed_files,
        changed_node_ids,
        affected_node_ids,
        warnings,
    };

    // Write to <repo>/.understand/diff-overlay.json (so the
    // dashboard server can serve it via /diff-overlay.json).
    let understand_dir = crate::commands::code_wiki_pipeline::understand_dir_for(&repo_dir);
    let target = understand_dir.join(DIFF_OVERLAY_FILE);
    let bytes = serde_json::to_vec_pretty(&overlay)
        .map_err(|e| format!("serialize diff overlay: {e}"))?;
    write_atomic(&target, &bytes)
        .map_err(|e| format!("write diff overlay: {e}"))?;

    Ok(Some(overlay))
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let (year, month, day, hour, min, sec) = epoch_to_ymdhms(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.000Z")
}

fn epoch_to_ymdhms(epoch: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = epoch.div_euclid(86_400);
    let secs_of_day = epoch.rem_euclid(86_400) as u32;
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
    (year, m, d, hour, min, sec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::{GraphEdge, GraphNode, ProjectMeta};

    fn n(id: &str, kind: &str, path: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            kind: kind.to_string(),
            name: path.rsplit_once('/').map(|(_, n)| n).unwrap_or(path).to_string(),
            file_path: path.to_string(),
            summary: String::new(),
            tags: vec![],
            complexity: "moderate".to_string(),
            location: None,
            language_notes: None,
        }
    }

    fn e(source: &str, target: &str) -> GraphEdge {
        GraphEdge {
            source: source.to_string(),
            target: target.to_string(),
            kind: "calls".to_string(),
            direction: "forward".to_string(),
            weight: 1.0,
        }
    }

    fn g() -> KnowledgeGraph {
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
            nodes: Vec::new(),
            edges: Vec::new(),
            layers: Vec::new(),
            tour: Vec::new(),
        }
    }

    #[test]
    fn compute_diff_finds_changed_file_nodes() {
        let mut graph = g();
        graph.nodes.push(n("file:src/foo.ts", "file", "src/foo.ts"));
        graph.nodes.push(n("file:src/bar.ts", "file", "src/bar.ts"));
        let (changed, affected) =
            compute_diff(&graph, &["src/foo.ts".to_string()]);
        assert_eq!(changed, vec!["file:src/foo.ts".to_string()]);
        assert!(affected.is_empty());
    }

    #[test]
    fn compute_diff_finds_function_nodes_in_changed_file() {
        let mut graph = g();
        graph.nodes.push(n("file:src/foo.ts", "file", "src/foo.ts"));
        graph.nodes.push(n("function:src/foo.ts:bar", "function", "src/foo.ts"));
        let (changed, _) = compute_diff(&graph, &["src/foo.ts".to_string()]);
        assert_eq!(changed.len(), 2);
    }

    #[test]
    fn compute_diff_walks_1_hop_in_both_directions() {
        let mut graph = g();
        graph.nodes.push(n("file:src/foo.ts", "file", "src/foo.ts"));
        graph.nodes.push(n("file:src/bar.ts", "file", "src/bar.ts"));
        graph.nodes.push(n("file:src/baz.ts", "file", "src/baz.ts"));
        // bar calls foo; foo imports baz
        graph.edges.push(e("file:src/bar.ts", "file:src/foo.ts"));
        graph.edges.push(e("file:src/foo.ts", "file:src/baz.ts"));
        let (changed, affected) =
            compute_diff(&graph, &["src/foo.ts".to_string()]);
        assert_eq!(changed, vec!["file:src/foo.ts".to_string()]);
        // Both neighbours (bar upstream, baz downstream) are affected
        assert_eq!(affected.len(), 2);
        assert!(affected.contains(&"file:src/bar.ts".to_string()));
        assert!(affected.contains(&"file:src/baz.ts".to_string()));
    }

    #[test]
    fn compute_diff_does_not_double_count_changed_node() {
        let mut graph = g();
        graph.nodes.push(n("file:src/foo.ts", "file", "src/foo.ts"));
        graph.nodes.push(n("file:src/bar.ts", "file", "src/bar.ts"));
        graph.edges.push(e("file:src/foo.ts", "file:src/bar.ts"));
        // Both changed
        let (changed, affected) = compute_diff(
            &graph,
            &["src/foo.ts".to_string(), "src/bar.ts".to_string()],
        );
        assert_eq!(changed.len(), 2);
        assert!(affected.is_empty(), "no affected when both endpoints changed");
    }

    #[test]
    fn compute_diff_normalises_paths() {
        let mut graph = g();
        graph.nodes.push(n("file:src/foo.ts", "file", "src/foo.ts"));
        let (changed, _) = compute_diff(&graph, &["./src/foo.ts".to_string()]);
        assert_eq!(changed, vec!["file:src/foo.ts".to_string()]);
    }

    #[test]
    fn compute_diff_handles_windows_separators() {
        let mut graph = g();
        graph.nodes.push(n("file:src/foo.ts", "file", "src/foo.ts"));
        let (changed, _) =
            compute_diff(&graph, &["src\\foo.ts".to_string()]);
        assert_eq!(changed, vec!["file:src/foo.ts".to_string()]);
    }

    #[test]
    fn compute_diff_returns_empty_when_no_changes_match() {
        let mut graph = g();
        graph.nodes.push(n("file:src/foo.ts", "file", "src/foo.ts"));
        let (changed, affected) =
            compute_diff(&graph, &["src/other.ts".to_string()]);
        assert!(changed.is_empty());
        assert!(affected.is_empty());
    }

    #[test]
    fn refresh_writes_overlay_file() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let repo = project.join("raw").join("code").join("gglog");
        let src = repo.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("lib.rs"), "pub fn x() {}\n").unwrap();
        // Pre-create a knowledge-graph.json (no git needed for this
        // assertion — the overlay should still be written even
        // when git isn't available; it just gets a warning).
        let repo_dir = crate::commands::code_wiki::repo_root(&project, "gglog");
        let graph = serde_json::json!({
            "version": "1.0.0",
            "kind": "codebase",
            "project": {
                "name": "gglog", "languages": ["rust"], "frameworks": [],
                "description": "", "analyzedAt": "", "gitCommitHash": ""
            },
            "nodes": [
                {"id": "file:src/lib.rs", "type": "file", "name": "lib.rs",
                 "filePath": "src/lib.rs", "summary": "", "tags": [],
                 "complexity": "moderate"}
            ],
            "edges": [],
            "layers": [], "tour": []
        });
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::fs::write(
            repo_dir.join("knowledge-graph.json"),
            serde_json::to_vec_pretty(&graph).unwrap(),
        ).unwrap();

        let result = refresh_diff_overlay_inner(&project, "gglog", None)
            .expect("refresh");
        // If git is available, we may or may not have an overlay
        // (depends on the temp dir's git state). We only assert
        // the file write side effect.
        let _ = result; // Either Some or None; both are OK for this test.
        let target = crate::commands::code_wiki_pipeline::understand_dir_for(&repo_dir)
            .join("diff-overlay.json");
        assert!(target.exists(), "overlay file should be written to disk");
        let raw = std::fs::read_to_string(&target).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["version"], "1.0.0");
        assert!(parsed["baseBranch"].is_string());
        assert!(parsed["generatedAt"].is_string());
    }
}

// --- Tauri commands ----------------------------------------------------

/// Tauri command: recompute the diff overlay for a repo. Returns
/// the new overlay (or `None` if git isn't available / the repo
/// isn't a git repo). Also writes the overlay to
/// `<repo>/.understand/diff-overlay.json` so the dashboard
/// server can serve it as `/diff-overlay.json`.
#[tauri::command]
pub async fn code_wiki_refresh_diff_overlay(
    project_path: String,
    repo_name: String,
    base: Option<String>,
) -> Result<Option<DiffOverlay>, String> {
    let pp = std::path::PathBuf::from(&project_path);
    tokio::task::spawn_blocking(move || {
        refresh_diff_overlay_inner(&pp, &repo_name, base.as_deref())
    })
    .await
    .map_err(|e| format!("diff overlay task panicked: {e}"))?
}

/// Tauri command: read the most recent diff overlay for a repo.
/// Returns `None` if the overlay hasn't been computed yet.
#[tauri::command]
pub async fn code_wiki_get_diff_overlay(
    project_path: String,
    repo_name: String,
) -> Result<Option<DiffOverlay>, String> {
    let pp = std::path::PathBuf::from(&project_path);
    tokio::task::spawn_blocking(move || {
        let repo_dir = crate::commands::code_wiki::repo_root(&pp, &repo_name);
        let understand_dir =
            crate::commands::code_wiki_pipeline::understand_dir_for(&repo_dir);
        let target = understand_dir.join(DIFF_OVERLAY_FILE);
        if !target.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&target)
            .map_err(|e| format!("read diff overlay: {e}"))?;
        serde_json::from_str(&raw)
            .map(Some)
            .map_err(|e| format!("parse diff overlay: {e}"))
    })
    .await
    .map_err(|e| format!("diff overlay task panicked: {e}"))?
}
