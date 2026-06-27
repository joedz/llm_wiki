use super::*;
use std::fs;
use std::path::PathBuf;

fn temp_root(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "code-wiki-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn codegraph_dir_for_lives_inside_wiki_code_wiki() {
    let project = temp_root("path");
    let dir = codegraph_dir_for(&project, "repo-A");
    assert!(dir.ends_with("wiki/code_wiki/repo-A/.codegraph"));
    let _ = fs::remove_dir_all(&project);
}

#[test]
fn is_code_wiki_public_path_accepts_graph_meta_index() {
    assert!(is_code_wiki_public_path("wiki/code_wiki/repo-A/graph.json"));
    assert!(is_code_wiki_public_path("wiki/code_wiki/index.json"));
    assert!(!is_code_wiki_public_path(
        "wiki/code_wiki/repo-A/.codegraph/codegraph.db"
    ));
    assert!(!is_code_wiki_public_path("wiki/index.md"));
}

#[test]
fn list_repos_returns_top_level_subdirs() {
    let project = temp_root("list");
    let code_root = project.join("raw").join("code");
    fs::create_dir_all(code_root.join("repo-A")).unwrap();
    fs::create_dir_all(code_root.join("repo-B")).unwrap();
    fs::create_dir_all(code_root.join(".cache")).unwrap();
    let names = list_repo_names(&project).unwrap();
    assert_eq!(names, vec!["repo-A".to_string(), "repo-B".to_string()]);
    let _ = fs::remove_dir_all(&project);
}

#[test]
fn read_index_returns_empty_when_missing() {
    let project = temp_root("empty-index");
    let index = read_or_empty_index(&project).unwrap();
    assert!(index.repos.is_empty());
    let _ = fs::remove_dir_all(&project);
}

#[test]
fn plan_index_command_uses_repo_path_not_codegraph_dir() {
    let project = temp_root("plan-index");
    let code_root = project.join("raw").join("code").join("repo-A");
    fs::create_dir_all(&code_root).unwrap();
    let plan = plan_index_invocation(&project, "repo-A");
    assert_eq!(plan.codegraph_dir, codegraph_dir_for(&project, "repo-A"));
    assert_eq!(plan.repo_root, code_root);
    let _ = fs::remove_dir_all(&project);
}

#[test]
fn affected_repos_from_changes_extracts_top_level_subdirs() {
    let changes = vec![
        "raw/code/repo-A/src/foo.ts".to_string(),
        "raw/code/repo-B/lib/bar.rs".to_string(),
        "raw/sources/notes.md".to_string(),
    ];
    let repos = affected_repos(&changes);
    assert_eq!(repos, vec!["repo-A".to_string(), "repo-B".to_string()]);
}
