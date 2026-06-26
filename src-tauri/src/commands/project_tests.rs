use super::create_project_impl;
use std::fs;

#[test]
fn create_project_creates_code_source_directory() {
    let root = std::env::temp_dir().join(format!("llm-wiki-project-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).expect("create temp root");

    let project = create_project_impl(
        "Code Sources".to_string(),
        root.to_string_lossy().to_string(),
    )
    .expect("create project");

    assert!(root.join("Code Sources/raw/code").is_dir());
    assert!(root.join("Code Sources/raw/sources").is_dir());
    assert!(project.path.ends_with("/Code Sources"));

    fs::remove_dir_all(root).expect("remove temp root");
}
