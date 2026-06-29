// Phase 4 — ARCHITECTURE. Assigns each file-level node to a
// layer. Mirrors UA's `architecture-analyzer` but uses a
// path-based heuristic (the M3 scope) instead of an LLM call —
// the LLM refinement is deferred to a follow-up. The heuristic
// is good enough for an MVP because the directory layout of
// most projects correlates strongly with their actual layers.
//
// Output shape matches UA's `Layer` schema: { id, name,
// description, nodeIds[] }. Every file-level node must belong
// to exactly one layer (UA invariant).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::commands::code_wiki_pipeline::KnowledgeGraph;

const MAX_LAYERS: usize = 12;
const MIN_FILES_PER_LAYER: usize = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchitectureReport {
    pub layers: Vec<Layer>,
    pub unassigned: u32,
}

/// Heuristic rules. For each path we compute a "layer key"
/// (the top-level dir under src/, or a path-pattern match for
/// common layouts). Files in the same dir get the same layer.
fn layer_key_for_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");

    // Common special paths
    if normalized.starts_with("src/api/") || normalized.starts_with("api/") {
        return "api".to_string();
    }
    if normalized.starts_with("src/db/") || normalized.starts_with("db/")
        || normalized.starts_with("src/database/") || normalized.starts_with("database/")
        || normalized.starts_with("migrations/") {
        return "data".to_string();
    }
    if normalized.starts_with("src/ui/") || normalized.starts_with("ui/")
        || normalized.starts_with("src/components/") || normalized.starts_with("components/")
        || normalized.starts_with("src/views/") || normalized.starts_with("views/")
        || normalized.starts_with("src/pages/") || normalized.starts_with("pages/") {
        return "ui".to_string();
    }
    if normalized.starts_with("src/services/") || normalized.starts_with("services/")
        || normalized.starts_with("src/handlers/") || normalized.starts_with("handlers/") {
        return "services".to_string();
    }
    if normalized.starts_with("src/utils/") || normalized.starts_with("utils/")
        || normalized.starts_with("src/lib/") || normalized.starts_with("lib/")
        || normalized.starts_with("src/helpers/") || normalized.starts_with("helpers/")
        || normalized.starts_with("src/util/") || normalized.starts_with("util/") {
        return "utilities".to_string();
    }
    if normalized.starts_with("src/types/") || normalized.starts_with("types/")
        || normalized.starts_with("src/models/") || normalized.starts_with("models/") {
        return "types".to_string();
    }
    if normalized.starts_with("src/hooks/") || normalized.starts_with("hooks/") {
        return "hooks".to_string();
    }
    if normalized.starts_with("src/store/") || normalized.starts_with("src/stores/")
        || normalized.starts_with("src/state/") || normalized.starts_with("state/") {
        return "state".to_string();
    }
    if normalized.starts_with("src/auth/") || normalized.starts_with("auth/") {
        return "auth".to_string();
    }
    if normalized.starts_with("src/config/") || normalized.starts_with("config/")
        || normalized.starts_with("src/settings/") || normalized.starts_with("settings/") {
        return "config".to_string();
    }
    if normalized.starts_with("tests/") || normalized.starts_with("test/")
        || normalized.starts_with("__tests__/") || normalized.starts_with("spec/") {
        return "tests".to_string();
    }
    if normalized.starts_with("scripts/") || normalized.starts_with("bin/") {
        return "scripts".to_string();
    }
    if normalized.starts_with("docs/") || normalized.starts_with("doc/") {
        return "docs".to_string();
    }
    if normalized.starts_with("examples/") || normalized.starts_with("example/")
        || normalized.starts_with("demo/") {
        return "examples".to_string();
    }

    // Detect common project layouts
    // 1. Monorepo: packages/<name>/ or apps/<name>/
    for prefix in ["packages/", "apps/", "services/", "modules/"] {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            // Take the first segment of the rest as the layer.
            let seg = rest.split('/').next().unwrap_or("root");
            return format!("{prefix}{seg}");
        }
    }
    // 2. Rust workspace: crates/<name>/
    if let Some(rest) = normalized.strip_prefix("crates/") {
        let seg = rest.split('/').next().unwrap_or("root");
        return format!("crates/{seg}");
    }
    // 3. src/<name>/ — common in React/Node projects
    if let Some(rest) = normalized.strip_prefix("src/") {
        let seg = rest.split('/').next().unwrap_or("root");
        return format!("src/{seg}");
    }
    // 4. lib/<name>/ — also common
    if let Some(rest) = normalized.strip_prefix("lib/") {
        let seg = rest.split('/').next().unwrap_or("root");
        return format!("lib/{seg}");
    }
    // 5. Flat: take the top-level dir as-is
    let top = normalized.split('/').next().unwrap_or("root");
    if top.is_empty() {
        "root".to_string()
    } else {
        top.to_string()
    }
}

fn layer_name_for_key(key: &str) -> String {
    match key {
        "api" => "API layer".to_string(),
        "data" => "Data layer".to_string(),
        "ui" => "UI layer".to_string(),
        "services" => "Services".to_string(),
        "utilities" => "Utilities".to_string(),
        "types" => "Types & models".to_string(),
        "hooks" => "Hooks".to_string(),
        "state" => "State management".to_string(),
        "auth" => "Auth".to_string(),
        "config" => "Configuration".to_string(),
        "tests" => "Tests".to_string(),
        "scripts" => "Scripts".to_string(),
        "docs" => "Docs".to_string(),
        "examples" => "Examples".to_string(),
        "root" => "Root".to_string(),
        other => {
            // For monorepo prefixes, prettify: "packages/api" -> "Package: api"
            for prefix in ["packages/", "apps/", "crates/", "src/", "lib/"] {
                if let Some(rest) = other.strip_prefix(prefix) {
                    return format!("{}: {}", prefix.trim_end_matches('/'), rest);
                }
            }
            other.to_string()
        }
    }
}

fn layer_description_for_key(key: &str) -> String {
    match key {
        "api" => "HTTP / RPC endpoints and request handlers.".to_string(),
        "data" => "Database, migrations, and persistence concerns.".to_string(),
        "ui" => "User-facing components, views, and styling.".to_string(),
        "services" => "Domain services and business logic handlers.".to_string(),
        "utilities" => "Shared helpers and low-level utilities.".to_string(),
        "types" => "Type definitions, schemas, and data models.".to_string(),
        "hooks" => "React-style lifecycle hooks.".to_string(),
        "state" => "Application state (stores, reducers, context).".to_string(),
        "auth" => "Authentication and authorization.".to_string(),
        "config" => "Configuration loading and environment wiring.".to_string(),
        "tests" => "Test suites and fixtures.".to_string(),
        "scripts" => "Operational scripts and CLIs.".to_string(),
        "docs" => "Documentation and guides.".to_string(),
        "examples" => "Example apps and demos.".to_string(),
        "root" => "Top-level project files (entry point, manifests).".to_string(),
        other => {
            if other.starts_with("packages/") || other.starts_with("apps/")
                || other.starts_with("crates/") {
                format!("Monorepo package `{}`.", other.split('/').nth(1).unwrap_or(other))
            } else {
                format!("Files under `{}`.", other)
            }
        }
    }
}

pub fn assign_layers(graph: &KnowledgeGraph) -> ArchitectureReport {
    // Group file-level nodes by their layer key.
    let mut by_key: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unassigned: u32 = 0;
    for node in &graph.nodes {
        // Only assign file-level nodes (file, config, document, etc.).
        // Skip function / class / etc. — they live inside file nodes.
        let is_file_level = matches!(
            node.kind.as_str(),
            "file" | "config" | "document" | "service" | "pipeline"
                | "table" | "schema" | "resource" | "endpoint"
        );
        if !is_file_level {
            continue;
        }
        if node.file_path.is_empty() {
            unassigned += 1;
            continue;
        }
        let key = layer_key_for_path(&node.file_path);
        by_key.entry(key).or_default().push(node.id.clone());
    }

    // Sort keys by file count desc (so the most important layers
    // come first), then alphabetically.
    let mut keys: Vec<(String, usize)> = by_key
        .iter()
        .map(|(k, v)| (k.clone(), v.len()))
        .collect();
    keys.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    // Truncate to MAX_LAYERS. The tail of small layers get
    // folded into a "misc" layer.
    let mut layers: Vec<Layer> = Vec::new();
    let mut misc_node_ids: Vec<String> = Vec::new();
    for (key, _count) in keys {
        let mut node_ids = by_key.remove(&key).unwrap_or_default();
        if layers.len() >= MAX_LAYERS {
            misc_node_ids.append(&mut node_ids);
            continue;
        }
        node_ids.sort();
        layers.push(Layer {
            id: format!("layer:{}", slugify(&key)),
            name: layer_name_for_key(&key),
            description: layer_description_for_key(&key),
            node_ids,
        });
    }
    if !misc_node_ids.is_empty() {
        misc_node_ids.sort();
        layers.push(Layer {
            id: "layer:misc".to_string(),
            name: "Miscellaneous".to_string(),
            description: "Files that don't fit any of the named layers above.".to_string(),
            node_ids: misc_node_ids,
        });
    }

    ArchitectureReport { layers, unassigned }
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::{GraphNode, ProjectMeta};

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

    fn g(nodes: Vec<GraphNode>) -> KnowledgeGraph {
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
            nodes,
            edges: vec![],
            layers: vec![],
            tour: vec![],
        }
    }

    #[test]
    fn groups_src_subdirs_into_layers() {
        let g = g(vec![
            n("file:src/api/users.ts", "file", "src/api/users.ts"),
            n("file:src/api/posts.ts", "file", "src/api/posts.ts"),
            n("file:src/db/schema.prisma", "file", "src/db/schema.prisma"),
            n("file:src/ui/Header.tsx", "file", "src/ui/Header.tsx"),
            n("file:src/utils/format.ts", "file", "src/utils/format.ts"),
        ]);
        let report = assign_layers(&g);
        let by_id: HashMap<&str, &Layer> =
            report.layers.iter().map(|l| (l.id.as_str(), l)).collect();
        assert!(by_id.contains_key("layer:api"), "got layers: {:?}", report.layers.iter().map(|l| &l.id).collect::<Vec<_>>());
        assert!(by_id.contains_key("layer:data"));
        assert!(by_id.contains_key("layer:ui"));
        assert!(by_id.contains_key("layer:utilities"));
    }

    #[test]
    fn treats_function_nodes_as_not_assignable() {
        let g = g(vec![
            n("file:src/api/users.ts", "file", "src/api/users.ts"),
            n("function:src/api/users.ts:create", "function", "src/api/users.ts"),
        ]);
        let report = assign_layers(&g);
        // Only the file node gets assigned.
        let api = report.layers.iter().find(|l| l.id == "layer:api").unwrap();
        assert_eq!(api.node_ids.len(), 1);
    }

    #[test]
    fn handles_monorepo_layout() {
        let g = g(vec![
            n("file:packages/web/src/index.ts", "file", "packages/web/src/index.ts"),
            n("file:packages/api/src/server.ts", "file", "packages/api/src/server.ts"),
        ]);
        let report = assign_layers(&g);
        let names: Vec<&str> = report.layers.iter().map(|l| l.name.as_str()).collect();
        // Should produce two package layers
        assert!(names.iter().any(|n| n.contains("web")), "got: {names:?}");
        assert!(names.iter().any(|n| n.contains("api")), "got: {names:?}");
    }

    #[test]
    fn caps_max_layers_and_folds_misc() {
        let mut nodes = Vec::new();
        // Generate 20 distinct layer keys
        for i in 0..20 {
            nodes.push(n(
                &format!("file:src/dir{i}/x.ts"),
                "file",
                &format!("src/dir{i}/x.ts"),
            ));
        }
        let g = g(nodes);
        let report = assign_layers(&g);
        assert!(report.layers.len() <= 13); // 12 + 1 misc
        // The last layer should be "miscellaneous"
        let last = report.layers.last().unwrap();
        assert_eq!(last.id, "layer:misc");
    }

    #[test]
    fn layer_node_ids_are_sorted() {
        let g = g(vec![
            n("file:src/api/z.ts", "file", "src/api/z.ts"),
            n("file:src/api/a.ts", "file", "src/api/a.ts"),
            n("file:src/api/m.ts", "file", "src/api/m.ts"),
        ]);
        let report = assign_layers(&g);
        let api = report.layers.iter().find(|l| l.id == "layer:api").unwrap();
        let ids: Vec<&str> = api.node_ids.iter().map(|s| s.as_str()).collect();
        assert_eq!(ids, vec!["file:src/api/a.ts", "file:src/api/m.ts", "file:src/api/z.ts"]);
    }

    #[test]
    fn layer_names_match_ua_style() {
        assert_eq!(layer_name_for_key("api"), "API layer");
        assert_eq!(layer_name_for_key("data"), "Data layer");
        assert_eq!(layer_name_for_key("ui"), "UI layer");
        assert_eq!(layer_name_for_key("utilities"), "Utilities");
        assert_eq!(layer_name_for_key("tests"), "Tests");
    }

    #[test]
    fn layer_descriptions_are_non_empty() {
        let g = g(vec![n("file:src/api/x.ts", "file", "src/api/x.ts")]);
        let report = assign_layers(&g);
        for layer in &report.layers {
            assert!(!layer.description.is_empty(), "empty description for {}", layer.id);
        }
    }
}
