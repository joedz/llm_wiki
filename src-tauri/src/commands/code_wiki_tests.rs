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
