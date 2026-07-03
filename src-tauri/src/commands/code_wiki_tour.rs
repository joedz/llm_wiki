// Phase 5 — TOUR. Builds a 5-7 step guided walkthrough of the
// project. Mirrors UA's `tour-builder` agent but uses a
// deterministic heuristic (the M3 scope) so the output is
// stable, testable, and doesn't burn LLM budget. The LLM
// refinement is deferred.
//
// Tour shape matches UA: { order, title, description, nodeIds[] }.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::commands::code_wiki_pipeline::KnowledgeGraph;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TourStep {
    pub order: u32,
    pub title: String,
    pub description: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TourReport {
    pub steps: Vec<TourStep>,
    pub truncated: bool,
}

const MAX_STEPS: usize = 8;
const MAX_NODES_PER_STEP: usize = 4;

pub fn build_tour(graph: &KnowledgeGraph) -> TourReport {
    let mut steps: Vec<TourStep> = Vec::new();
    // Track which node IDs we've already shown so the same file
    // doesn't appear in two steps. (Some tests cover this case
    // where the same file is entry + complex, for example.)
    let mut shown: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Step 1: project entry points (top-level src/main.*, src/index.*, lib.rs)
    let entry = filter_unshown(&pick_entry_files(graph), &mut shown);
    if !entry.is_empty() {
        steps.push(TourStep {
            order: 1,
            title: "Project entry point".to_string(),
            description: "Start here. These files are the application's entry point — \
                         the function that boots the runtime, parses config, and \
                         wires the system together."
                .to_string(),
            node_ids: take_ids(&entry, 2),
        });
    }

    // Step 2: top-level architecture (most-imported files)
    let hub = filter_unshown(&pick_hub_files(graph), &mut shown);
    if !hub.is_empty() {
        steps.push(TourStep {
            order: 2,
            title: "Architecture hubs".to_string(),
            description: "The most-imported files in the codebase. Other modules \
                         depend heavily on these — they encode the project's \
                         core abstractions."
                .to_string(),
            node_ids: take_ids(&hub, 3),
        });
    }

    // Step 3: API / service surface
    let api = filter_unshown(&pick_api_files(graph), &mut shown);
    if !api.is_empty() {
        steps.push(TourStep {
            order: 3,
            title: "Public API surface".to_string(),
            description: "Routes, handlers, and entry points that other systems \
                         or users call. Read these to understand what the \
                         program can do."
                .to_string(),
            node_ids: take_ids(&api, 3),
        });
    }

    // Step 4: data layer
    let data = filter_unshown(&pick_data_files(graph), &mut shown);
    if !data.is_empty() {
        steps.push(TourStep {
            order: 4,
            title: "Data & persistence".to_string(),
            description: "Schemas, migrations, repositories. Where state lives \
                         and how it changes."
                .to_string(),
            node_ids: take_ids(&data, 2),
        });
    }

    // Step 5: complex / risk hotspots
    let complex = filter_unshown(&pick_complex_files(graph), &mut shown);
    if !complex.is_empty() {
        steps.push(TourStep {
            order: 5,
            title: "Complexity hotspots".to_string(),
            description: "Files marked complex by the LLM (or that have many \
                         edges). Good places to read carefully when making \
                         changes — they're load-bearing and easy to break."
                .to_string(),
            node_ids: take_ids(&complex, 2),
        });
    }

    // Step 6: tests
    let tests = filter_unshown(&pick_test_files(graph), &mut shown);
    if !tests.is_empty() {
        steps.push(TourStep {
            order: 6,
            title: "Test surface".to_string(),
            description: "How the project validates its behaviour. Pick one to \
                         see the test style and naming conventions."
                .to_string(),
            node_ids: take_ids(&tests, 2),
        });
    }

    // Step 7: build & config
    let build = filter_unshown(&pick_build_files(graph), &mut shown);
    if !build.is_empty() && steps.len() < MAX_STEPS {
        steps.push(TourStep {
            order: 7,
            title: "Build & configuration".to_string(),
            description: "Manifests and build scripts. Useful when wiring up a \
                         new environment or troubleshooting build issues."
                .to_string(),
            node_ids: take_ids(&build, 2),
        });
    }

    // Renumber sequentially (in case we skipped some)
    for (i, step) in steps.iter_mut().enumerate() {
        step.order = (i + 1) as u32;
    }

    let truncated = steps.len() > MAX_STEPS;
    if truncated {
        steps.truncate(MAX_STEPS);
    }

    TourReport { steps, truncated }
}

fn filter_unshown(
    ids: &[String],
    shown: &mut std::collections::HashSet<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for id in ids {
        if shown.insert(id.clone()) {
            out.push(id.clone());
        }
    }
    out
}

fn take_ids(ids: &[String], max: usize) -> Vec<String> {
    let mut out: Vec<String> = ids.iter().take(max).cloned().collect();
    out.sort();
    out
}

/// Files that look like entry points: src/main.{ts,rs,go,py,java},
/// src/index.{ts,js}, src/App.tsx, lib.rs, main.go, __main__.py.
fn pick_entry_files(graph: &KnowledgeGraph) -> Vec<String> {
    const ENTRY_NAMES: &[&str] = &[
        "main.ts", "main.rs", "main.go", "main.py", "Main.java", "main.java",
        "index.ts", "index.js", "index.tsx", "App.tsx", "App.jsx",
        "lib.rs", "lib.rs", "__main__.py", "manage.py", "app.py",
        "wsgi.py", "asgi.py", "Program.cs",
    ];
    graph.nodes.iter()
        .filter(|n| n.kind == "file" && ENTRY_NAMES.iter().any(|e| n.file_path.ends_with(e) || n.file_path.ends_with(&format!("/{e}"))))
        .map(|n| n.id.clone())
        .collect()
}

fn pick_hub_files(graph: &KnowledgeGraph) -> Vec<String> {
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for e in &graph.edges {
        if e.kind != "imports" { continue; }
        *counts.entry(e.target.clone()).or_insert(0) += 1;
    }
    let mut hubs: Vec<(String, u32)> = counts.into_iter().filter(|(_, c)| *c >= 2).collect();
    hubs.sort_by(|a, b| b.1.cmp(&a.1));
    hubs.into_iter().map(|(id, _)| id).collect()
}

fn pick_api_files(graph: &KnowledgeGraph) -> Vec<String> {
    const API_SUFFIXES: &[&str] = &["routes", "router", "handlers", "controller", "endpoint"];
    graph.nodes.iter()
        .filter(|n| {
            (n.kind == "file" || n.kind == "endpoint")
                && API_SUFFIXES.iter().any(|s| n.file_path.to_lowercase().contains(s))
        })
        .map(|n| n.id.clone())
        .collect()
}

fn pick_data_files(graph: &KnowledgeGraph) -> Vec<String> {
    graph.nodes.iter()
        .filter(|n| matches!(n.kind.as_str(), "schema" | "table" | "document" | "file")
            && (n.file_path.contains("schema") || n.file_path.contains("migration")
                || n.file_path.contains("model") || n.file_path.contains("repository")))
        .map(|n| n.id.clone())
        .collect()
}

fn pick_complex_files(graph: &KnowledgeGraph) -> Vec<String> {
    graph.nodes.iter()
        .filter(|n| n.complexity == "complex" && n.kind == "file")
        .map(|n| n.id.clone())
        .collect()
}

fn pick_test_files(graph: &KnowledgeGraph) -> Vec<String> {
    graph.nodes.iter()
        .filter(|n| {
            n.kind == "file" && (
                n.file_path.contains("test/") || n.file_path.contains("tests/")
                    || n.file_path.contains("__tests__/") || n.file_path.contains("spec/")
                    || n.file_path.ends_with(".test.ts") || n.file_path.ends_with(".test.js")
                    || n.file_path.ends_with(".spec.ts") || n.file_path.ends_with(".spec.js")
                    || n.file_path.ends_with("_test.rs") || n.file_path.ends_with("_spec.rs")
            )
        })
        .map(|n| n.id.clone())
        .collect()
}

fn pick_build_files(graph: &KnowledgeGraph) -> Vec<String> {
    const BUILD_NAMES: &[&str] = &[
        "package.json", "Cargo.toml", "pyproject.toml", "go.mod",
        "pom.xml", "build.gradle", "tsconfig.json", "vite.config",
        "webpack.config.js", "rollup.config.js", "Dockerfile",
        "docker-compose.yml", "Makefile", ".github/workflows/ci.yml",
    ];
    graph.nodes.iter()
        .filter(|n| BUILD_NAMES.iter().any(|b| n.file_path.ends_with(b) || n.file_path.ends_with(&format!("/{b}"))))
        .map(|n| n.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_pipeline::{GraphEdge, GraphNode, ProjectMeta};

    fn n(id: &str, kind: &str, path: &str, complexity: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            kind: kind.to_string(),
            name: path.rsplit_once('/').map(|(_, n)| n).unwrap_or(path).to_string(),
            file_path: path.to_string(),
            summary: String::new(),
            tags: vec![],
            complexity: complexity.to_string(),
            location: None,
            language_notes: None,
        }
    }

    fn e(source: &str, target: &str) -> GraphEdge {
        GraphEdge {
            source: source.to_string(),
            target: target.to_string(),
            kind: "imports".to_string(),
            direction: "forward".to_string(),
            weight: 1.0,
            ..Default::default()
        }
    }

    fn g(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> KnowledgeGraph {
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
            edges,
            layers: vec![],
            tour: vec![],
        }
    }

    #[test]
    fn picks_entry_points() {
        let g = g(
            vec![
                n("file:src/main.ts", "file", "src/main.ts", "moderate"),
                n("file:src/utils/format.ts", "file", "src/utils/format.ts", "moderate"),
            ],
            vec![],
        );
        let report = build_tour(&g);
        assert!(report.steps.iter().any(|s| s.title == "Project entry point"));
    }

    #[test]
    fn picks_hubs_from_import_counts() {
        // Many files import core.ts; core.ts is a hub.
        let g = g(
            vec![
                n("file:src/core.ts", "file", "src/core.ts", "moderate"),
                n("file:src/a.ts", "file", "src/a.ts", "moderate"),
                n("file:src/b.ts", "file", "src/b.ts", "moderate"),
                n("file:src/c.ts", "file", "src/c.ts", "moderate"),
            ],
            vec![
                e("file:src/a.ts", "file:src/core.ts"),
                e("file:src/b.ts", "file:src/core.ts"),
                e("file:src/c.ts", "file:src/core.ts"),
            ],
        );
        let report = build_tour(&g);
        let hub = report.steps.iter().find(|s| s.title == "Architecture hubs");
        assert!(hub.is_some(), "no hub step: {:?}", report.steps.iter().map(|s| &s.title).collect::<Vec<_>>());
        assert!(hub.unwrap().node_ids.contains(&"file:src/core.ts".to_string()));
    }

    #[test]
    fn picks_complex_files() {
        let g = g(
            vec![
                n("file:src/normal.ts", "file", "src/normal.ts", "moderate"),
                n("file:src/complex.ts", "file", "src/complex.ts", "complex"),
            ],
            vec![],
        );
        let report = build_tour(&g);
        let step = report.steps.iter().find(|s| s.title == "Complexity hotspots");
        assert!(step.is_some());
        assert!(step.unwrap().node_ids.contains(&"file:src/complex.ts".to_string()));
    }

    #[test]
    fn picks_test_files() {
        let g = g(
            vec![
                n("file:tests/foo.test.ts", "file", "tests/foo.test.ts", "moderate"),
                n("file:src/main.ts", "file", "src/main.ts", "moderate"),
            ],
            vec![],
        );
        let report = build_tour(&g);
        let step = report.steps.iter().find(|s| s.title == "Test surface");
        assert!(step.is_some());
        assert!(step.unwrap().node_ids.contains(&"file:tests/foo.test.ts".to_string()));
    }

    #[test]
    fn step_orders_are_sequential() {
        let g = g(
            vec![
                n("file:src/main.ts", "file", "src/main.ts", "moderate"),
                n("file:src/core.ts", "file", "src/core.ts", "complex"),
                n("file:tests/foo.test.ts", "file", "tests/foo.test.ts", "moderate"),
            ],
            vec![],
        );
        let report = build_tour(&g);
        let orders: Vec<u32> = report.steps.iter().map(|s| s.order).collect();
        let mut expected: Vec<u32> = (1..=orders.len() as u32).collect();
        assert_eq!(orders, expected);
    }

    #[test]
    fn handles_minimal_graph() {
        let g = g(vec![], vec![]);
        let report = build_tour(&g);
        assert!(report.steps.is_empty());
        assert!(!report.truncated);
    }

    #[test]
    fn no_duplicate_node_ids_across_steps() {
        let g = g(
            vec![
                n("file:src/main.ts", "file", "src/main.ts", "complex"),
                n("file:src/main.ts.test", "file", "src/main.ts.test", "moderate"),
            ],
            vec![],
        );
        let report = build_tour(&g);
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for step in &report.steps {
            for id in &step.node_ids {
                assert!(seen.insert(id.clone()), "duplicate {id} in tour");
            }
        }
    }
}
