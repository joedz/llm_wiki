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
fn codegraph_dir_for_lives_next_to_source() {
    // codegraph 0.9.x always writes its DB to `<path>/.codegraph/`, so
    // for an imported repo at raw/code/<repo> the DB lives there too.
    let project = temp_root("path");
    let dir = codegraph_dir_for(&project, "repo-A");
    assert!(dir.ends_with("raw/code/repo-A/.codegraph"));
    let _ = fs::remove_dir_all(&project);
}

#[test]
fn is_code_wiki_public_path_accepts_graph_meta_index() {
    assert!(is_code_wiki_public_path("wiki/code_wiki/repo-A/knowledge-graph.json"));
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

#[test]
fn parse_codegraph_context_json_to_payload_keeps_required_fields() {
    let raw = br#"{
      "languages": ["typescript"],
      "nodes": [{"id": "file:src/a.ts", "type": "file", "name": "a.ts", "filePath": "src/a.ts", "tags": []}],
      "edges": []
    }"#;
    let payload: CodegraphContextPayload = serde_json::from_slice(raw).unwrap();
    assert_eq!(payload.languages, vec!["typescript".to_string()]);
    assert_eq!(payload.nodes.len(), 1);
}

#[test]
fn read_db_payload_reads_real_codegraph_db() {
    // Build a real codegraph DB in a temp dir to exercise the rusqlite path.
    let project = temp_root("real-db");
    let repo = project.join("raw").join("code").join("gglog");
    let src = repo.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("lib.rs"), "pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();

    let bin = match which::which("codegraph") {
        Ok(b) => b,
        Err(_) => {
            // codegraph CLI not installed in this environment — skip the
            // e2e half; the unit tests above already cover the parse path.
            eprintln!("codegraph not on PATH; skipping read_db_payload integration test");
            let _ = fs::remove_dir_all(&project);
            return;
        }
    };

    let init_status = std::process::Command::new(&bin)
        .arg("init")
        .arg(&repo)
        .status()
        .expect("spawn codegraph init");
    assert!(init_status.success(), "codegraph init failed: {:?}", init_status);
    let index_status = std::process::Command::new(&bin)
        .arg("index")
        .arg(&repo)
        .status()
        .expect("spawn codegraph index");
    assert!(index_status.success(), "codegraph index failed: {:?}", index_status);

    let plan = plan_index_invocation(&project, "gglog");
    assert!(plan.codegraph_db.exists(), "codegraph DB not created");

    let payload = run_get_graph_payload_inner(&project, "gglog").expect("read payload");
    assert!(
        !payload.nodes.is_empty(),
        "expected nodes from real codegraph DB, got 0"
    );
    // The Rust function in lib.rs should be present
    let has_add = payload
        .nodes
        .iter()
        .any(|n| n.kind == "function" && n.name == "add");
    assert!(has_add, "function 'add' missing from real DB payload");

    // JSON shape must match what the TS exporter expects
    let json = serde_json::to_string(&payload).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse");
    let first = parsed["nodes"][0].as_object().expect("node object");
    assert!(first.contains_key("filePath"), "missing filePath: {}", json);
    assert!(first.contains_key("type"), "missing type: {}", json);

    let _ = fs::remove_dir_all(&project);
}
