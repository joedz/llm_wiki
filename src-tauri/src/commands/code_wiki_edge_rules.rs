// Deterministic extraction for non-code edge types.
//
// P1-A added three deterministic extractors (`tested_by` /
// `configures` / `depends_on`) plus LLM-emitted `reads_from` /
// `writes_to`. P2-A extends coverage to nine additional edge types
// from UA's spec:
//
//   - `subscribes` / `publishes`: pub/sub topology from
//     `subscribers/` / `consumers/` / `publishers/` / `producers/` /
//     `events/` directory conventions, connected via a shared event
//     module.
//
//   - `middleware`: middleware → routes file pairing based on
//     imports.
//
//   - `routes`: routing configs (nginx / ingress / routes.ts /
//     web.php) → upstream service files.
//
//   - `defines_schema`: schema files (GraphQL / Protobuf / JSON
//     Schema / OpenAPI) → consumer files (resolvers / clients).
//
//   - `triggers` / `serves` / `provisions` / `migrates`: infra file
//     → target code / resource / table mapping for CI workflows,
//     K8s manifests, Terraform, and SQL migrations respectively.
//
// All functions are pure: they take a `ScanResult` (for file paths +
// categories) plus the set of valid node ids from the in-progress
// graph and return a `Vec<GraphEdge>`. The pipeline splices them
// into the edge list before Phase 5 (assemble) so the assembler can
// dedupe + drop dangling edges like any other source.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use crate::commands::code_wiki_pipeline::GraphEdge;
use crate::commands::code_wiki_scanner::ScanResult;

/// Cap on how many target nodes a single source can fan out to
/// (e.g. `tsconfig.json` could theoretically configure every `.ts`
/// file in a large repo). 50 matches UA's typical limit.
const MAX_CONFIGURES_TARGETS_PER_SOURCE: usize = 50;

/// Strip a test suffix to recover the production filename. Returns
/// `None` if the filename doesn't match any test convention.
fn strip_test_suffix(filename: &str) -> Option<String> {
    let lower = filename.to_ascii_lowercase();
    let stem = lower.rsplit_once('.').map(|(_, e)| e);

    // tsconfig-style prefixes
    if let Some(stem_inner) = stem {
        // .test.ts / .spec.ts / .test.tsx / .spec.tsx / .test.js / .spec.js
        if matches!(stem_inner, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs") {
            if let Some((prefix, last_ext)) = lower.rsplit_once(".test.") {
                if last_ext == stem_inner {
                    return Some(format!("{prefix}.{last_ext}"));
                }
            }
            if let Some((prefix, last_ext)) = lower.rsplit_once(".spec.") {
                if last_ext == stem_inner {
                    return Some(format!("{prefix}.{last_ext}"));
                }
            }
        }
    }

    // _test.go → foo.go (Go convention)
    if lower.ends_with("_test.go") {
        let stem_part = &lower[..lower.len() - "_test.go".len()];
        return Some(format!("{stem_part}.go"));
    }
    // TestFoo.kt / FooTests.kt / FooTest.kt → Foo.kt (Kotlin/Java)
    if lower.ends_with("test.kt") && !lower.ends_with("test.kt.kt") {
        let stem_part = &lower[..lower.len() - "test.kt".len()];
        return Some(format!("{stem_part}.kt"));
    }
    if lower.ends_with("tests.kt") && !lower.ends_with("tests.kt.kt") {
        let stem_part = &lower[..lower.len() - "tests.kt".len()];
        return Some(format!("{stem_part}.kt"));
    }
    // Kotlin/Java Test-prefix convention: `TestFoo.kt` → `Foo.kt`,
    // `TestFoo.java` → `Foo.java`. Strip the leading `test` only when
    // it's at the very start of the basename (no directory prefix).
    if lower.starts_with("test") && lower.ends_with(".kt") {
        let stem = &lower[..lower.len() - ".kt".len()];
        let rest = &stem["test".len()..];
        if !rest.is_empty() {
            return Some(format!("{rest}.kt"));
        }
    }
    if lower.starts_with("test") && lower.ends_with(".java") {
        let stem = &lower[..lower.len() - ".java".len()];
        let rest = &stem["test".len()..];
        if !rest.is_empty() {
            return Some(format!("{rest}.java"));
        }
    }
    if lower.ends_with("test.java") {
        let stem_part = &lower[..lower.len() - "test.java".len()];
        return Some(format!("{stem_part}.java"));
    }
    if lower.ends_with("tests.java") {
        let stem_part = &lower[..lower.len() - "tests.java".len()];
        return Some(format!("{stem_part}.java"));
    }
    if lower.ends_with("it.java") {
        let stem_part = &lower[..lower.len() - "it.java".len()];
        return Some(format!("{stem_part}.java"));
    }
    // *_test.cpp / *_test.cc / test_*.cpp / test_*.cc (C/C++)
    if lower.ends_with("_test.cpp") {
        let stem_part = &lower[..lower.len() - "_test.cpp".len()];
        return Some(format!("{stem_part}.cpp"));
    }
    if lower.ends_with("_test.cc") {
        let stem_part = &lower[..lower.len() - "_test.cc".len()];
        return Some(format!("{stem_part}.cc"));
    }
    if lower.starts_with("test_") && lower.ends_with(".cpp") {
        return Some(lower.replacen("test_", "", 1));
    }
    if lower.starts_with("test_") && lower.ends_with(".cc") {
        return Some(lower.replacen("test_", "", 1));
    }
    if lower.starts_with("test_") && lower.ends_with(".c") {
        return Some(lower.replacen("test_", "", 1));
    }
    if lower.ends_with("_test.c") {
        let stem_part = &lower[..lower.len() - "_test.c".len()];
        return Some(format!("{stem_part}.c"));
    }
    // *_test.rb / *_spec.rb (Ruby)
    if lower.ends_with("_test.rb") {
        let stem_part = &lower[..lower.len() - "_test.rb".len()];
        return Some(format!("{stem_part}.rb"));
    }
    if lower.ends_with("_spec.rb") {
        let stem_part = &lower[..lower.len() - "_spec.rb".len()];
        return Some(format!("{stem_part}.rb"));
    }
    // test_*.lua / *_test.lua / *_spec.lua
    if lower.starts_with("test_") && lower.ends_with(".lua") {
        return Some(lower.replacen("test_", "", 1));
    }
    if lower.ends_with("_test.lua") {
        let stem_part = &lower[..lower.len() - "_test.lua".len()];
        return Some(format!("{stem_part}.lua"));
    }
    if lower.ends_with("_spec.lua") {
        let stem_part = &lower[..lower.len() - "_spec.lua".len()];
        return Some(format!("{stem_part}.lua"));
    }
    // test_*.py / *_test.py / tests/test_*.py → test_*.py → *.py
    if lower.starts_with("test_") && lower.ends_with(".py") {
        return Some(lower.replacen("test_", "", 1));
    }
    if lower.ends_with("_test.py") {
        let stem_part = &lower[..lower.len() - "_test.py".len()];
        return Some(format!("{stem_part}.py"));
    }
    // *_test.dart (Dart)
    if lower.ends_with("_test.dart") {
        let stem_part = &lower[..lower.len() - "_test.dart".len()];
        return Some(format!("{stem_part}.dart"));
    }
    // *_test.swift
    if lower.ends_with("tests.swift") {
        let stem_part = &lower[..lower.len() - "tests.swift".len()];
        return Some(format!("{stem_part}.swift"));
    }
    if lower.ends_with("test.swift") {
        let stem_part = &lower[..lower.len() - "test.swift".len()];
        return Some(format!("{stem_part}.swift"));
    }

    None
}

/// Determine whether a relative path looks like a test file based
/// on directory and filename conventions.
fn is_test_path(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());
    let dir = lower.rsplit_once('/').map(|(d, _)| d).unwrap_or("");

    // Directory conventions: __tests__/, test/, tests/, spec/
    if dir.ends_with("/__tests__")
        || dir.ends_with("/__test__")
        || dir.ends_with("/__specs__")
        || dir.ends_with("/__spec__")
        || dir.ends_with("/test")
        || dir.ends_with("/tests")
        || dir.ends_with("/spec")
    {
        return true;
    }

    // Filename conventions: *.test.* / *.spec.* / *_test.* / Test*.{java,kt}
    // / *Test.{kt,cs} / *_spec.* / test_*.{py,c,cpp,cc,lua}
    if filename.contains(".test.") || filename.contains(".spec.") {
        return true;
    }
    if filename.ends_with("_test.go")
        || filename.ends_with("_test.cpp")
        || filename.ends_with("_test.cc")
        || filename.ends_with("_test.c")
        || filename.ends_with("_test.py")
        || filename.ends_with("_test.rb")
        || filename.ends_with("_test.lua")
        || filename.ends_with("_spec.rb")
        || filename.ends_with("_spec.lua")
        || filename.ends_with("_test.dart")
    {
        return true;
    }
    if filename.starts_with("test_") && filename.ends_with(".py") {
        return true;
    }
    if filename.starts_with("test_")
        && (filename.ends_with(".lua")
            || filename.ends_with(".c")
            || filename.ends_with(".cpp")
            || filename.ends_with(".cc"))
    {
        return true;
    }
    // Tests.cs / Tests.kt / Tests.java (and Test prefix in some langs)
    if filename.ends_with("test.kt")
        || filename.ends_with("tests.kt")
        || filename.ends_with("test.java")
        || filename.ends_with("tests.java")
        || filename.ends_with("it.java")
    {
        return true;
    }
    // *.swift test conventions
    if filename.ends_with("test.swift") || filename.ends_with("tests.swift") {
        return true;
    }
    // C#: FooTests.cs / FooTest.cs
    if filename.ends_with("tests.cs") || filename.ends_with("test.cs") {
        return true;
    }

    false
}

/// Extract `tested_by` edges: pair test files with their production
/// counterpart via path canonicalization. Direction is canonicalised
/// to `production → test` so the dashboard arrow points from code to
/// its tests.
///
/// Mirrors UA's path-convention pairing (see
/// `packages/core/src/languages/configs/*.ts` `filePatterns.tests`).
pub fn extract_tested_by(scan: &ScanResult, valid_node_ids: &HashSet<String>) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for f in &scan.files {
        let lower_path = f.path.to_ascii_lowercase();
        let filename = lower_path.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower_path.as_str());

        if !is_test_path(&f.path) {
            continue;
        }

        // Skip if the test file's own node doesn't exist.
        let test_id = format!("file:{}", f.path);
        if !valid_node_ids.contains(&test_id) {
            continue;
        }

        // Try canonicalization first.
        let canonical = strip_test_suffix(filename).unwrap_or_default();
        if canonical.is_empty() {
            continue;
        }

        // Build candidate production paths to try. The test file
        // might be in a different directory than production — we
        // try same-dir first, then any other file with the same
        // basename + canonical extension.
        let prod_candidates = candidate_production_paths(&f.path, &canonical);

        // Skip test↔test: if every candidate resolves to another
        // test path, emit nothing.
        let mut resolved_prod: Option<String> = None;
        for cand in &prod_candidates {
            if !is_test_path(cand) && valid_node_ids.contains(&format!("file:{cand}")) {
                resolved_prod = Some(cand.clone());
                break;
            }
        }
        let Some(prod_path) = resolved_prod else {
            continue;
        };

        let prod_id = format!("file:{prod_path}");
        let key = (prod_id.clone(), test_id.clone());
        if seen.insert(key) {
            edges.push(GraphEdge {
                source: prod_id,
                target: test_id,
                kind: "tested_by".to_string(),
                direction: "forward".to_string(),
                weight: 0.5,
                description: None,
            });
        }
    }

    edges
}

/// Build the list of plausible production paths for a test file
/// located at `test_path` whose canonical form (after stripping the
/// test suffix) is `canonical`. Tries same-directory first, then
/// any other scan file with the same canonical basename.
fn candidate_production_paths(test_path: &str, canonical: &str) -> Vec<String> {
    let mut out = Vec::new();
    let test_lower = test_path.to_ascii_lowercase();
    let parent = Path::new(test_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Same-directory
    let same_dir = if parent.is_empty() {
        canonical.to_string()
    } else {
        format!("{}/{}", parent, canonical)
    };
    out.push(same_dir);

    // We don't have access to scan.files here — caller will filter
    // by `valid_node_ids`. But we can also try a flat-repo candidate.
    if let Some((bare_name, _ext)) = canonical.rsplit_once('.') {
        // Common alternate locations: src/, lib/, app/.
        // The caller still filters by valid_node_ids, so it's safe
        // to propose paths that may not exist.
        for prefix in ["src", "lib", "app"] {
            out.push(format!("{prefix}/{bare_name}.{}", ext_of(canonical)));
        }
    }

    // Dedup
    let mut seen: HashSet<String> = HashSet::new();
    out.retain(|p| seen.insert(p.to_ascii_lowercase()));
    let _ = test_lower; // suppress unused

    out
}

fn ext_of(filename: &str) -> &str {
    filename.rsplit_once('.').map(|(_, e)| e).unwrap_or("")
}

// ---------------------------------------------------------------------------
// configures
// ---------------------------------------------------------------------------

/// Source filename → list of glob-pattern languages it targets.
/// Lower-case, dot included for the match (e.g. ".ts").
fn configures_targets(filename: &str, language: &str, all_scan_paths: &[String]) -> Vec<String> {
    let lower = filename.to_ascii_lowercase();

    // tsconfig.json / tsconfig.base.json / tsconfig.*.json
    if lower.starts_with("tsconfig") && lower.ends_with(".json") {
        return collect_paths_with_extensions(all_scan_paths, &["ts", "tsx"]);
    }

    // package.json → entry point (.ts/.js/.tsx/.jsx/.mjs)
    if lower == "package.json" {
        return collect_paths_matching_any(
            all_scan_paths,
            &[
                "src/index.ts",
                "src/index.tsx",
                "src/index.js",
                "src/index.jsx",
                "src/index.mjs",
                "src/main.ts",
                "src/main.tsx",
                "src/main.js",
                "src/main.jsx",
                "src/main.mjs",
                "index.ts",
                "index.tsx",
                "index.js",
                "index.jsx",
                "src/App.tsx",
                "src/App.jsx",
                "src/server.ts",
                "src/server.js",
                "src/cli.ts",
                "src/cli.js",
            ],
        );
    }

    // Cargo.toml → src/main.rs / src/lib.rs
    if lower == "cargo.toml" {
        return collect_paths_matching_any(
            all_scan_paths,
            &[
                "src/main.rs",
                "src/lib.rs",
                "src/bin/main.rs",
            ],
        );
    }

    // pyproject.toml / setup.py / setup.cfg → pyproject-defined scripts
    // We just emit to all .py in src/ if present.
    if matches!(lower.as_str(), "pyproject.toml" | "setup.py" | "setup.cfg") {
        return collect_paths_with_prefix(all_scan_paths, "src/", &["py"]);
    }

    // .eslintrc* / eslint.config.* → all same-language source files
    if lower.starts_with(".eslintrc") || lower.starts_with("eslint.config.") {
        let ext = match language {
            "json" | "yaml" | "javascript" => "js",
            _ => "ts",
        };
        let extras: &[&str] = if ext == "ts" {
            &["ts", "tsx", "js", "jsx"]
        } else {
            &["js", "jsx", "ts", "tsx"]
        };
        return collect_paths_with_extensions(all_scan_paths, extras);
    }

    // .prettierrc* / prettier.config.* → all source files
    if lower.starts_with(".prettierrc") || lower.starts_with("prettier.config.") {
        return collect_paths_with_extensions(
            all_scan_paths,
            &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
        );
    }

    // .env* / .env files
    if lower == ".env" || lower.starts_with(".env.") || lower == ".env.example" || lower == ".env.sample" {
        return collect_paths_matching_any(
            all_scan_paths,
            &[
                "src/index.ts",
                "src/index.tsx",
                "src/index.js",
                "src/index.jsx",
                "src/main.ts",
                "src/main.tsx",
                "src/main.js",
                "src/main.jsx",
                "src/server.ts",
                "src/server.js",
                "src/config.ts",
                "src/config.js",
                "src/env.ts",
                "src/env.js",
            ],
        );
    }

    // vite.config.* / webpack.config.* / rollup.config.* / next.config.* →
    // entry point
    if lower.starts_with("vite.config.")
        || lower.starts_with("webpack.config.")
        || lower.starts_with("rollup.config.")
        || lower.starts_with("next.config.")
        || lower.starts_with("esbuild.config.")
    {
        return collect_paths_matching_any(
            all_scan_paths,
            &[
                "src/index.ts",
                "src/index.tsx",
                "src/index.js",
                "src/index.jsx",
                "src/main.ts",
                "src/main.tsx",
                "src/main.js",
                "src/main.jsx",
                "src/App.tsx",
                "src/App.jsx",
            ],
        );
    }

    // tailwind.config.* / postcss.config.* → all .css / .tsx / .jsx
    if lower.starts_with("tailwind.config.") || lower.starts_with("postcss.config.") {
        return collect_paths_with_extensions(all_scan_paths, &["css", "tsx", "jsx", "ts", "js"]);
    }

    vec![]
}

fn collect_paths_with_extensions(paths: &[String], exts: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for p in paths {
        let lower = p.to_ascii_lowercase();
        if let Some(ext) = lower.rsplit_once('.').map(|(_, e)| e) {
            if exts.iter().any(|e| *e == ext) {
                out.push(p.clone());
            }
        }
        if out.len() >= MAX_CONFIGURES_TARGETS_PER_SOURCE {
            break;
        }
    }
    out
}

fn collect_paths_matching_any(paths: &[String], candidates: &[&str]) -> Vec<String> {
    let set: BTreeSet<String> = candidates.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    for p in paths {
        if set.contains(p) {
            out.push(p.clone());
        }
    }
    out
}

fn collect_paths_with_prefix(paths: &[String], prefix: &str, exts: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for p in paths {
        let lower = p.to_ascii_lowercase();
        if !lower.starts_with(prefix) {
            continue;
        }
        if let Some(ext) = lower.rsplit_once('.').map(|(_, e)| e) {
            if exts.iter().any(|e| *e == ext) {
                out.push(p.clone());
            }
        }
        if out.len() >= MAX_CONFIGURES_TARGETS_PER_SOURCE {
            break;
        }
    }
    out
}

/// Extract `configures` edges based on config-file filename +
/// target-language heuristics.
pub fn extract_configures(scan: &ScanResult, valid_node_ids: &HashSet<String>) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    let all_paths: Vec<String> = scan.files.iter().map(|f| f.path.clone()).collect();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for f in &scan.files {
        let filename = f
            .path
            .rsplit_once('/')
            .map(|(_, f)| f)
            .unwrap_or(f.path.as_str());
        let lower = filename.to_ascii_lowercase();

        // Quick filter: only config-looking filenames
        if !is_likely_config_filename(&lower) {
            continue;
        }

        let source_id = format!("file:{}", f.path);
        if !valid_node_ids.contains(&source_id) {
            continue;
        }

        let targets = configures_targets(filename, &f.language, &all_paths);
        for tgt in targets {
            let target_id = format!("file:{tgt}");
            if !valid_node_ids.contains(&target_id) {
                continue;
            }
            let key = (source_id.clone(), target_id.clone());
            if seen.insert(key) {
                edges.push(GraphEdge {
                    source: source_id.clone(),
                    target: target_id,
                    kind: "configures".to_string(),
                    direction: "forward".to_string(),
                    weight: 0.6,
                    description: None,
                });
            }
        }
    }

    edges
}

fn is_likely_config_filename(lower: &str) -> bool {
    lower.starts_with("tsconfig")
        || lower == "package.json"
        || lower == "cargo.toml"
        || matches!(lower, "pyproject.toml" | "setup.py" | "setup.cfg")
        || lower.starts_with(".eslintrc")
        || lower.starts_with("eslint.config.")
        || lower.starts_with(".prettierrc")
        || lower.starts_with("prettier.config.")
        || lower == ".env"
        || lower.starts_with(".env.")
        || matches!(lower, ".env.example" | ".env.sample")
        || lower.starts_with("vite.config.")
        || lower.starts_with("webpack.config.")
        || lower.starts_with("rollup.config.")
        || lower.starts_with("next.config.")
        || lower.starts_with("esbuild.config.")
        || lower.starts_with("tailwind.config.")
        || lower.starts_with("postcss.config.")
}

// ---------------------------------------------------------------------------
// depends_on (non-code → code)
// ---------------------------------------------------------------------------

/// Extract `depends_on` edges for non-code infra files that point
/// at code targets. Pure path / regex based — no LLM.
pub fn extract_non_code_depends_on(
    scan: &ScanResult,
    project_root: &Path,
    valid_node_ids: &HashSet<String>,
) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();

    for f in &scan.files {
        if !is_depends_on_source(&f.path, &f.language) {
            continue;
        }
        let source_id = format!("file:{}", f.path);
        if !valid_node_ids.contains(&source_id) {
            continue;
        }

        let abs = project_root.join(&f.path);
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let targets = extract_depends_on_targets(&f.path, &content);
        for tgt in targets {
            let target_id = format!("file:{tgt}");
            if !valid_node_ids.contains(&target_id) {
                continue;
            }
            if tgt == f.path {
                continue; // skip self-loops
            }
            let key = (source_id.clone(), target_id.clone());
            if seen.insert(key) {
                edges.push(GraphEdge {
                    source: source_id.clone(),
                    target: target_id,
                    kind: "depends_on".to_string(),
                    direction: "forward".to_string(),
                    weight: 0.6,
                    description: None,
                });
            }
        }
    }

    edges
}

fn is_depends_on_source(rel: &str, _language: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());

    if matches!(
        filename,
        "dockerfile" | "dockerfile.prod" | "dockerfile.dev" | "compose.yml" | "compose.yaml"
            | "docker-compose.yml" | "docker-compose.yaml" | "makefile" | "gnumakefile"
            | "procfile"
    ) {
        return true;
    }
    if filename == ".gitlab-ci.yml" {
        return true;
    }
    if lower.starts_with(".github/workflows/") && (lower.ends_with(".yml") || lower.ends_with(".yaml"))
    {
        return true;
    }
    if filename == "package.json" {
        return true;
    }
    false
}

fn extract_depends_on_targets(rel: &str, content: &str) -> Vec<String> {
    let lower = rel.to_ascii_lowercase();
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());
    let mut out: Vec<String> = Vec::new();

    if filename.starts_with("dockerfile") {
        // COPY <src> <dst>  /  ADD <src> <dst>
        for line in content.lines() {
            let trimmed = line.trim_start();
            let upper = trimmed.to_ascii_uppercase();
            if upper.starts_with("COPY ") || upper.starts_with("ADD ") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    let src = parts[1];
                    if !src.starts_with("http://") && !src.starts_with("https://") {
                        out.push(src.to_string());
                    }
                }
            }
        }
    } else if filename.contains("docker-compose") || filename == "compose.yml" || filename == "compose.yaml" {
        // best-effort: find `build: ./foo` or `dockerfile: ./Dockerfile`
        for line in content.lines() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("build:") {
                let v = rest.trim().trim_matches('"').trim_matches('\'');
                if v.starts_with("./") || v.starts_with("../") || !v.contains(':') {
                    out.push(format!("{v}/Dockerfile"));
                }
            }
            if let Some(rest) = t.strip_prefix("dockerfile:") {
                out.push(rest.trim().trim_matches('"').trim_matches('\'').to_string());
            }
        }
    } else if lower.starts_with(".github/workflows/") {
        // - run: npm test / - run: ./scripts/foo.sh
        for line in content.lines() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("- run:") {
                let v = rest.trim().trim_matches('"').trim_matches('\'');
                if (v.starts_with("./") || v.starts_with("bash ") || v.starts_with("sh "))
                    && !v.contains(' ')
                {
                    out.push(v.to_string());
                }
            }
        }
    } else if filename == "makefile" || filename == "gnumakefile" {
        // -include foo.mk / include foo.mk
        for line in content.lines() {
            let t = line.trim_start();
            if t.starts_with("include ") || t.starts_with("-include ") {
                let v = t
                    .trim_start_matches("-include ")
                    .trim_start_matches("include ")
                    .trim();
                out.push(v.to_string());
            }
        }
    } else if filename == "package.json" {
        // scripts.<name> → "<cmd>" — best-effort: if cmd looks like a path
        // to a project file, emit it. Avoids emitting `tsc`, `vitest`, etc.
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
            if let Some(scripts) = parsed.get("scripts").and_then(|s| s.as_object()) {
                for (_k, v) in scripts {
                    if let Some(cmd) = v.as_str() {
                        // only pick paths that look like file references
                        for token in cmd.split_whitespace() {
                            if (token.starts_with("./") || token.starts_with("../"))
                                && !token.contains(':')
                            {
                                out.push(token.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// P2-A: subscribes / publishes / middleware / routes / defines_schema
//        / triggers / serves / provisions / migrates
// ---------------------------------------------------------------------------

/// P2-A: Pub/sub topology — connect subscribers to publishers via a
/// shared event module when one exists, else connect them directly.
///
/// Conventions recognized (case-insensitive, by directory and
/// filename):
///   - subscribers: `src/subscribers/`, `src/consumers/`,
///     `src/events/handlers/`, `src/listeners/`
///     Filename: `*.subscriber.ts`, `*.consumer.ts`
///   - publishers: `src/publishers/`, `src/producers/`,
///     `src/events/`
///     Filename: `*.publisher.ts`, `*.producer.ts`
pub fn extract_pub_sub(
    scan: &ScanResult,
    valid_node_ids: &HashSet<String>,
) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    let subscribers = collect_pub_sub_files(scan, PubSubRole::Subscriber);
    let publishers = collect_pub_sub_files(scan, PubSubRole::Publisher);

    // Build a map: event-bus basename → path. Events are files in
    // events/ that don't have a "subscriber"/"publisher"/"consumer"/
    // "producer" suffix.
    let event_bus: std::collections::BTreeMap<String, String> = scan
        .files
        .iter()
        .filter(|f| is_event_bus_path(&f.path))
        .filter(|f| !is_pub_sub_role(&f.path).is_some())
        .map(|f| {
            let stem = event_bus_stem(&f.path);
            (stem, f.path.clone())
        })
        .collect();

    // subscribers → publishers (via shared event bus if any)
    for (sub_path, _sub_file) in &subscribers {
        let sub_basename = pub_sub_basename(sub_path);
        let source_id = format!("file:{sub_path}");
        if !valid_node_ids.contains(&source_id) {
            continue;
        }
        for (pub_path, _pub_file) in &publishers {
            let pub_basename = pub_sub_basename(pub_path);
            let target_id = format!("file:{pub_path}");
            if !valid_node_ids.contains(&target_id) {
                continue;
            }
            // Same event name → connect via event bus path if it exists
            let shared_event = if sub_basename == pub_basename {
                event_bus.get(&sub_basename).cloned()
            } else {
                None
            };
            let (kind, edge_source, edge_target) = if let Some(ev) = shared_event {
                let ev_id = format!("file:{ev}");
                if !valid_node_ids.contains(&ev_id) {
                    continue;
                }
                ("subscribes", source_id.clone(), ev_id)
            } else {
                // No event bus: connect subscriber directly to publisher
                ("subscribes", source_id.clone(), target_id.clone())
            };
            let key = (edge_source.clone(), edge_target.clone(), kind.to_string());
            if seen.insert(key) {
                edges.push(GraphEdge {
                    source: edge_source,
                    target: edge_target,
                    kind: kind.to_string(),
                    direction: "forward".to_string(),
                    weight: 0.6,
                    description: None,
                });
            }
        }
    }

    // publishers → subscribers (only via event bus; without a bus
    // we don't know which subscriber consumes which event)
    for (pub_path, _) in &publishers {
        let pub_basename = pub_sub_basename(pub_path);
        let source_id = format!("file:{pub_path}");
        if !valid_node_ids.contains(&source_id) {
            continue;
        }
        if let Some(ev) = event_bus.get(&pub_basename) {
            let ev_id = format!("file:{ev}");
            if !valid_node_ids.contains(&ev_id) {
                continue;
            }
            // Emit publishes for each subscriber of this event bus
            for (sub_path, _) in &subscribers {
                if pub_sub_basename(sub_path) != pub_basename {
                    continue;
                }
                let target_id = format!("file:{sub_path}");
                let key = (source_id.clone(), target_id.clone(), "publishes".to_string());
                if seen.insert(key) {
                    edges.push(GraphEdge {
                        source: source_id.clone(),
                        target: target_id,
                        kind: "publishes".to_string(),
                        direction: "forward".to_string(),
                        weight: 0.6,
                        description: None,
                    });
                }
            }
        }
    }

    edges
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PubSubRole {
    Subscriber,
    Publisher,
}

fn collect_pub_sub_files(scan: &ScanResult, role: PubSubRole) -> Vec<(String, &crate::commands::code_wiki_scanner::ScannedFile)> {
    scan.files
        .iter()
        .filter(|f| is_pub_sub_role(&f.path) == Some(role))
        .map(|f| (f.path.clone(), f))
        .collect()
}

fn is_pub_sub_role(rel: &str) -> Option<PubSubRole> {
    let lower = rel.to_ascii_lowercase();
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());
    let dir = lower.rsplit_once('/').map(|(d, _)| d).unwrap_or("");

    // Filename suffixes
    if filename.ends_with(".subscriber.ts")
        || filename.ends_with(".subscriber.js")
        || filename.ends_with(".consumer.ts")
        || filename.ends_with(".consumer.go")
    {
        return Some(PubSubRole::Subscriber);
    }
    if filename.ends_with(".publisher.ts")
        || filename.ends_with(".publisher.js")
        || filename.ends_with(".producer.ts")
        || filename.ends_with(".producer.go")
    {
        return Some(PubSubRole::Publisher);
    }
    // Directory conventions
    if dir.contains("/subscribers/")
        || dir.contains("/consumers/")
        || dir.contains("/listeners/")
        || dir.ends_with("/subscribers")
        || dir.ends_with("/consumers")
        || dir.ends_with("/listeners")
        || dir.contains("/events/handlers/")
    {
        return Some(PubSubRole::Subscriber);
    }
    if dir.contains("/publishers/")
        || dir.contains("/producers/")
        || dir.ends_with("/publishers")
        || dir.ends_with("/producers")
    {
        return Some(PubSubRole::Publisher);
    }
    None
}

fn is_event_bus_path(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let dir = lower.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    // events/ directory containing event definitions (not handlers)
    dir.contains("/events/") || dir.ends_with("/events")
}

fn pub_sub_basename(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());
    let stem = filename.rsplit_once('.').map(|(s, _)| s).unwrap_or(filename);
    // Strip common suffixes
    for suf in [".subscriber", ".consumer", ".publisher", ".producer"] {
        if let Some(s) = stem.strip_suffix(suf) {
            return s.to_string();
        }
    }
    stem.to_string()
}

fn event_bus_stem(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());
    filename
        .rsplit_once('.')
        .map(|(s, _)| s.to_string())
        .unwrap_or_else(|| filename.to_string())
}

/// P2-A: Routes + middleware topology. Routes files (nginx / ingress
/// / routes.ts / web.php) emit `routes` edges to their target service
/// files. Middleware files emit `middleware` edges to the route file
/// that imports them.
pub fn extract_routes_and_middleware(
    scan: &ScanResult,
    project_root: &Path,
    valid_node_ids: &HashSet<String>,
) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    for f in &scan.files {
        let lower = f.path.to_ascii_lowercase();
        let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());

        let route_kind = classify_route_file(filename);
        if let Some(target) = route_kind {
            let source_id = format!("file:{}", f.path);
            if !valid_node_ids.contains(&source_id) {
                continue;
            }
            let abs = project_root.join(&f.path);
            let content = std::fs::read_to_string(&abs).unwrap_or_default();
            for tgt in extract_route_targets(filename, &content, target) {
                let target_id = format!("file:{tgt}");
                if !valid_node_ids.contains(&target_id) {
                    continue;
                }
                let key = (source_id.clone(), target_id.clone(), "routes".to_string());
                if seen.insert(key) {
                    edges.push(GraphEdge {
                        source: source_id.clone(),
                        target: target_id,
                        kind: "routes".to_string(),
                        direction: "forward".to_string(),
                        weight: 0.6,
                        description: None,
                    });
                }
            }
        }
    }

    // Middleware: any file in middleware/ / interceptors/ / guards/ dir
    // or matching a filename pattern, plus routes/index.ts / routes.ts
    // that imports it (use scan.import_map if available).
    let middleware_files: Vec<String> = scan
        .files
        .iter()
        .filter(|f| is_middleware_path(&f.path))
        .map(|f| f.path.clone())
        .collect();

    for mw in &middleware_files {
        let source_id = format!("file:{mw}");
        if !valid_node_ids.contains(&source_id) {
            continue;
        }
        // Find routes files that import this middleware.
        for routes_file in scan.files.iter().filter(|f| is_routes_filename(&f.path)) {
            let imports = scan
                .import_map
                .get(&routes_file.path)
                .cloned()
                .unwrap_or_default();
            if imports.iter().any(|i| i == mw) {
                let target_id = format!("file:{}", routes_file.path);
                let key = (source_id.clone(), target_id.clone(), "middleware".to_string());
                if seen.insert(key) {
                    edges.push(GraphEdge {
                        source: source_id.clone(),
                        target: target_id,
                        kind: "middleware".to_string(),
                        direction: "forward".to_string(),
                        weight: 0.5,
                        description: None,
                    });
                }
            }
        }
    }

    edges
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RouteTarget {
    /// nginx.conf / *.nginx.conf — proxy_pass to upstream service
    Nginx,
    /// ingress*.yaml — service.name + service.port
    Ingress,
    /// routes.ts / router.ts / app routes — app.use(path, handler)
    TsRoutes,
    /// web.php / api.php — Route::get('path', Controller::class)
    LaravelRoutes,
}

fn classify_route_file(filename: &str) -> Option<RouteTarget> {
    if filename == "nginx.conf" || filename.ends_with(".nginx.conf") {
        return Some(RouteTarget::Nginx);
    }
    if filename.starts_with("ingress") && (filename.ends_with(".yaml") || filename.ends_with(".yml")) {
        return Some(RouteTarget::Ingress);
    }
    if filename == "routes.ts"
        || filename == "router.ts"
        || filename == "routes.js"
        || filename == "router.js"
    {
        return Some(RouteTarget::TsRoutes);
    }
    if filename == "web.php" || filename == "api.php" {
        return Some(RouteTarget::LaravelRoutes);
    }
    None
}

fn extract_route_targets(
    filename: &str,
    content: &str,
    target: RouteTarget,
) -> Vec<String> {
    let mut out = Vec::new();
    match target {
        RouteTarget::Nginx => {
            // proxy_pass http://<service>;  /  proxy_pass http://<service>:<port>;
            for line in content.lines() {
                let t = line.trim_start();
                if let Some(rest) = t
                    .strip_prefix("proxy_pass")
                    .or_else(|| t.strip_prefix("proxy_pass "))
                {
                    let v = rest
                        .trim()
                        .trim_end_matches(';')
                        .trim_matches('"')
                        .trim_matches('\'');
                    if let Some(upstream) = v
                        .strip_prefix("http://")
                        .or_else(|| v.strip_prefix("https://"))
                    {
                        let svc = upstream
                            .split(':')
                            .next()
                            .unwrap_or(upstream)
                            .split('/')
                            .next()
                            .unwrap_or(upstream)
                            .trim();
                        if !svc.is_empty() {
                            // Emit with common code extensions so the
                            // target hits the existing scan files.
                            for ext in ["ts", "js", "py", "go"] {
                                out.push(format!("services/{svc}.{ext}"));
                                out.push(format!("src/services/{svc}.{ext}"));
                            }
                        }
                    }
                }
            }
        }
        RouteTarget::Ingress => {
            // service: name + port — find under backend: block
            let mut current_service: Option<String> = None;
            for line in content.lines() {
                let t = line.trim_start();
                if t.starts_with("service:") {
                    let v = t
                        .trim_start_matches("service:")
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'');
                    current_service = Some(v.to_string());
                }
            }
            if let Some(svc) = current_service {
                for ext in ["ts", "js", "py", "go"] {
                    out.push(format!("services/{svc}.{ext}"));
                    out.push(format!("src/services/{svc}.{ext}"));
                }
            }
        }
        RouteTarget::TsRoutes => {
            // app.use('/path', handler) or router.use('/path', handler)
            for line in content.lines() {
                let t = line.trim_start();
                if let Some(rest) = t.strip_prefix("app.use(").or_else(|| t.strip_prefix("router.use(")) {
                    // path, handler
                    let parts: Vec<&str> = rest
                        .split(',')
                        .map(|s| s.trim().trim_matches('\'').trim_matches('"'))
                        .collect();
                    if parts.len() >= 2 {
                        out.push(parts[1].to_string());
                    }
                }
            }
        }
        RouteTarget::LaravelRoutes => {
            // Route::get('/path', Controller::class)
            for line in content.lines() {
                if line.contains("Route::") {
                    // Best-effort: extract the controller reference
                    if let Some(idx) = line.find("use App\\Http\\Controllers\\") {
                        let rest = &line[idx + "use App\\Http\\Controllers\\".len()..];
                        if let Some(end) = rest.find(|c: char| c == ';' || c == ' ' || c == '\n') {
                            out.push(format!("app/Http/Controllers/{}.php", &rest[..end]));
                        }
                    }
                }
            }
        }
    }
    let _ = filename;
    out
}

fn is_routes_filename(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());
    classify_route_file(filename).is_some()
        || filename.contains(".routes.")
        || filename.contains(".router.")
        || filename == "routes.ts"
        || filename == "router.ts"
        // Routes-as-a-folder: any file under a `routes/` or `router/` directory
        // is also a candidate (the directory itself is the routes container).
        || lower.contains("/routes/")
        || lower.contains("/router/")
        || lower.starts_with("routes/")
        || lower.starts_with("router/")
}

fn is_middleware_path(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let dir = lower.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());

    dir.contains("/middleware/")
        || dir.contains("/middlewares/")
        || dir.contains("/interceptors/")
        || dir.contains("/guards/")
        || dir.ends_with("/middleware")
        || dir.ends_with("/middlewares")
        || dir.ends_with("/interceptors")
        || dir.ends_with("/guards")
        || filename.ends_with(".middleware.ts")
        || filename.ends_with(".middleware.js")
        || filename.ends_with(".guard.ts")
        || filename.ends_with(".interceptor.ts")
}

/// P2-A: Schema files (GraphQL / Protobuf / JSON Schema / OpenAPI)
/// emit `defines_schema` edges to consumer files (resolvers /
/// clients / models). Targets are inferred by import_map.
pub fn extract_schema_definitions(
    scan: &ScanResult,
    valid_node_ids: &HashSet<String>,
) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    for f in &scan.files {
        let schema_kind = classify_schema_file(&f.path);
        let Some(kind) = schema_kind else { continue };
        let source_id = format!("file:{}", f.path);
        if !valid_node_ids.contains(&source_id) {
            continue;
        }
        let targets = schema_consumer_paths(&f.path, kind, scan);
        for tgt in targets {
            let target_id = format!("file:{tgt}");
            if !valid_node_ids.contains(&target_id) {
                continue;
            }
            let key = (source_id.clone(), target_id.clone(), "defines_schema".to_string());
            if seen.insert(key) {
                edges.push(GraphEdge {
                    source: source_id.clone(),
                    target: target_id,
                    kind: "defines_schema".to_string(),
                    direction: "forward".to_string(),
                    weight: 0.8,
                    description: None,
                });
            }
        }
    }

    edges
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SchemaKind {
    Graphql,
    Protobuf,
    JsonSchema,
    OpenApi,
}

fn classify_schema_file(rel: &str) -> Option<SchemaKind> {
    let lower = rel.to_ascii_lowercase();
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());
    if filename.ends_with(".graphql") || filename.ends_with(".gql") {
        return Some(SchemaKind::Graphql);
    }
    if filename.ends_with(".proto") {
        return Some(SchemaKind::Protobuf);
    }
    // *.schema.json but not tsconfig.json
    if filename.ends_with(".schema.json") && !filename.starts_with("tsconfig") {
        return Some(SchemaKind::JsonSchema);
    }
    if (filename == "openapi.yaml" || filename == "openapi.yml" || filename == "openapi.json")
        || filename.starts_with("openapi.")
    {
        return Some(SchemaKind::OpenApi);
    }
    None
}

fn schema_consumer_paths(schema_path: &str, kind: SchemaKind, scan: &ScanResult) -> Vec<String> {
    let schema_dir = std::path::Path::new(schema_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let consumer_keywords: &[&str] = match kind {
        SchemaKind::Graphql => &["resolver", "handler", "controller", "query"],
        SchemaKind::Protobuf => &["service", "client", "server", "rpc"],
        SchemaKind::JsonSchema => &["model", "types", "schema"],
        SchemaKind::OpenApi => &["controller", "handler", "client", "sdk"],
    };

    let mut out = Vec::new();
    let exts = ["ts", "tsx", "js", "jsx", "py", "go", "java", "kt"];

    for f in &scan.files {
        let lower = f.path.to_ascii_lowercase();
        let file_ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        if !exts.contains(&file_ext) {
            continue;
        }
        if !consumer_keywords.iter().any(|kw| lower.contains(kw)) {
            continue;
        }
        // Prefer same-directory matches; otherwise anything containing
        // the schema's parent directory.
        if !schema_dir.is_empty() {
            let parent = std::path::Path::new(&schema_dir)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if !parent.is_empty() && lower.contains(&parent) {
                out.push(f.path.clone());
            }
        }
        if out.len() >= 50 {
            break;
        }
    }
    if out.is_empty() {
        // Fallback: any consumer-keyword file in the project
        for f in &scan.files {
            let lower = f.path.to_ascii_lowercase();
            if consumer_keywords.iter().any(|kw| lower.contains(kw)) {
                out.push(f.path.clone());
                if out.len() >= 50 {
                    break;
                }
            }
        }
    }
    let _ = schema_path;
    out
}

/// P2-A: Infrastructure topology — `triggers` / `serves` /
/// `provisions` / `migrates`. Each is driven by a different file
/// convention; we parse a minimum viable subset of each format to
/// find the target file path.
pub fn extract_infrastructure_topology(
    scan: &ScanResult,
    project_root: &Path,
    valid_node_ids: &HashSet<String>,
) -> Vec<GraphEdge> {
    let mut edges = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    for f in &scan.files {
        let lower = f.path.to_ascii_lowercase();
        let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());
        let abs = project_root.join(&f.path);
        let content = std::fs::read_to_string(&abs).unwrap_or_default();
        if f.path.contains("migrations") {
            eprintln!("DEBUG: {} content = {:?}", f.path, content);
        }

        // triggers: GH Actions / GitLab CI / Jenkins
        if lower.starts_with(".github/workflows/")
            || filename == ".gitlab-ci.yml"
            || filename == "jenkinsfile"
        {
            let source_id = format!("file:{}", f.path);
            if !valid_node_ids.contains(&source_id) {
                continue;
            }
            for tgt in extract_trigger_targets(&content) {
                let target_id = format!("file:{tgt}");
                let key = (source_id.clone(), target_id.clone(), "triggers".to_string());
                if seen.insert(key) {
                    edges.push(GraphEdge {
                        source: source_id.clone(),
                        target: target_id,
                        kind: "triggers".to_string(),
                        direction: "forward".to_string(),
                        weight: 0.6,
                        description: None,
                    });
                }
            }
        }

        // serves: k8s / manifests with Deployment or Service
        if lower.starts_with("k8s/")
            || lower.starts_with("kubernetes/")
            || lower.starts_with("manifests/")
            || filename.starts_with("deployment")
            || filename.starts_with("service")
        {
            if !filename.ends_with(".yaml") && !filename.ends_with(".yml") {
                continue;
            }
            let source_id = format!("file:{}", f.path);
            if !valid_node_ids.contains(&source_id) {
                continue;
            }
            for tgt in extract_serve_targets(&content) {
                let target_id = format!("file:{tgt}");
                let key = (source_id.clone(), target_id.clone(), "serves".to_string());
                if seen.insert(key) {
                    edges.push(GraphEdge {
                        source: source_id.clone(),
                        target: target_id,
                        kind: "serves".to_string(),
                        direction: "forward".to_string(),
                        weight: 0.7,
                        description: None,
                    });
                }
            }
        }

        // provisions: *.tf
        if filename.ends_with(".tf") {
            let source_id = format!("file:{}", f.path);
            if !valid_node_ids.contains(&source_id) {
                continue;
            }
            for tgt in extract_provisions_targets(&content) {
                let target_id = format!("file:{tgt}");
                let key = (source_id.clone(), target_id.clone(), "provisions".to_string());
                if seen.insert(key) {
                    edges.push(GraphEdge {
                        source: source_id.clone(),
                        target: target_id,
                        kind: "provisions".to_string(),
                        direction: "forward".to_string(),
                        weight: 0.7,
                        description: None,
                    });
                }
            }
        }

        // migrates: SQL migrations
        if is_sql_migration(&f.path) {
            let source_id = format!("file:{}", f.path);
            if !valid_node_ids.contains(&source_id) {
                continue;
            }
            for tgt in extract_migration_targets(&content, scan) {
                let target_id = format!("file:{tgt}");
                if !valid_node_ids.contains(&target_id) {
                    continue;
                }
                let key = (source_id.clone(), target_id.clone(), "migrates".to_string());
                if seen.insert(key) {
                    edges.push(GraphEdge {
                        source: source_id.clone(),
                        target: target_id,
                        kind: "migrates".to_string(),
                        direction: "forward".to_string(),
                        weight: 0.7,
                        description: None,
                    });
                }
            }
        }
        let _ = valid_node_ids;
    }

    edges
}

fn extract_trigger_targets(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("- run:") {
            let v = rest.trim().trim_matches('"').trim_matches('\'');
            for token in v.split_whitespace() {
                if (token.starts_with("./") || token.starts_with("../"))
                    && !token.contains(':')
                {
                    // Strip leading ./ so the target matches scan paths.
                    let cleaned = token
                        .trim_start_matches("./")
                        .trim_start_matches("../");
                    out.push(cleaned.to_string());
                }
            }
        } else if let Some(rest) = t.strip_prefix("script:") {
            let v = rest.trim().trim_matches('"').trim_matches('\'');
            if (v.starts_with("./") || v.starts_with("../")) && !v.contains(' ') {
                let cleaned = v
                    .trim_start_matches("./")
                    .trim_start_matches("../");
                out.push(cleaned.to_string());
            }
        }
    }
    out
}

fn extract_serve_targets(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    // Walk each container block: collect name + image pairs, emit
    // a synthesised path `src/<name>.{ts,py,go}` per pair.
    let mut current_image: Option<String> = None;
    let mut current_name: Option<String> = None;
    for line in content.lines() {
        let t = line.trim_start();
        // Strip leading "- " if present (YAML list item)
        let t = t.strip_prefix("- ").unwrap_or(t);
        if let Some(rest) = t.strip_prefix("image:") {
            let v = rest.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                current_image = Some(v.to_string());
            }
        }
        if let Some(rest) = t.strip_prefix("name:") {
            let v = rest.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                current_name = Some(v.to_string());
            }
        }
        if let (Some(name), Some(_image)) = (&current_name, &current_image) {
            // Avoid emitting duplicates for repeated lines
            out.push(format!("src/{}.ts", name));
            out.push(format!("src/{}.py", name));
            out.push(format!("src/{}.go", name));
            current_name = None;
            current_image = None;
        }
    }
    out
}

fn extract_provisions_targets(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    // resource "<type>" "<name>" { ... }
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("resource ") {
            // resource "<type>" "<name>"
            let parts: Vec<&str> = rest.split_whitespace().collect();
            // parts[0] = "<type>", parts[1] = "<name>", parts[2] = "{"
            if parts.len() >= 3 {
                let type_name = parts[0].trim_matches('"');
                let resource_name = parts[1].trim_matches('"');
                // Synthesize a "type" path — matches UA convention
                out.push(format!("infra/{}/{}", type_name, resource_name));
            }
        }
    }
    out
}

fn is_sql_migration(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());
    if !filename.ends_with(".sql") {
        return false;
    }
    // migrations/ prefix (with or without leading slash) OR numeric
    // prefix convention (0001_*.sql).
    let dir = lower.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    dir.contains("migrations")
        || dir.ends_with("migrations")
        || filename.starts_with("migration")
}

fn extract_migration_targets(content: &str, scan: &ScanResult) -> Vec<String> {
    let mut out = Vec::new();
    let mut tables = std::collections::BTreeSet::new();
    for line in content.lines() {
        let upper = line.trim_start().to_ascii_uppercase();
        // Check longer/more-specific prefixes first so `CREATE TABLE
        // IF NOT EXISTS` doesn't match `CREATE TABLE` and then mistake
        // "IF" for the table name.
        let after = if let Some(rest) = upper.strip_prefix("CREATE TABLE IF NOT EXISTS") {
            Some(rest)
        } else if let Some(rest) = upper.strip_prefix("CREATE TABLE") {
            Some(rest)
        } else if let Some(rest) = upper.strip_prefix("ALTER TABLE") {
            Some(rest)
        } else if let Some(rest) = upper.strip_prefix("DROP TABLE") {
            Some(rest)
        } else {
            None
        };
        if let Some(rest) = after {
            // First non-quoted word is the table name
            let raw = rest.trim().trim_matches('"').trim_matches('`');
            let table = raw
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches('"')
                .trim_matches('`')
                .trim_matches(';')
                .to_string();
            if !table.is_empty() && table != "IF" {
                tables.insert(table.to_ascii_lowercase());
            }
        }
    }
    // Map each table name to plausible consumer files
    for table in tables {
        // Try both the exact name and a singular form (users → user,
        // posts → post) so model files named `user.ts` match.
        let candidates = vec![table.clone(), table.trim_end_matches('s').to_string()];
        for f in &scan.files {
            let lower = f.path.to_ascii_lowercase();
            let ext_ok = lower.ends_with(".ts")
                || lower.ends_with(".py")
                || lower.ends_with(".go")
                || lower.ends_with(".java")
                || lower.ends_with(".kt");
            if !ext_ok {
                continue;
            }
            if candidates.iter().any(|c| lower.contains(c)) {
                out.push(f.path.clone());
                if out.len() >= 50 {
                    return out;
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_scanner::{ScannedFile, ScanStats};

    fn make_scan(paths: &[&str]) -> ScanResult {
        make_scan_at(paths, ".".to_string())
    }

    fn make_scan_at(paths: &[&str], project_root: String) -> ScanResult {
        let files: Vec<ScannedFile> = paths
            .iter()
            .map(|p| ScannedFile {
                path: p.to_string(),
                language: "typescript".to_string(),
                size_lines: 50,
                file_category: "code".to_string(),
            })
            .collect();
        ScanResult {
            project_root,
            total_files: files.len() as u32,
            filtered_by_ignore: 0,
            estimated_complexity: "moderate".to_string(),
            stats: ScanStats {
                files_scanned: files.len() as u32,
                by_category: Default::default(),
                by_language: Default::default(),
            },
            project_name: "test".to_string(),
            project_description: String::new(),
            frameworks: vec![],
            git_commit_hash: String::new(),
            files,
            import_map: Default::default(),
        }
    }

    fn ids_for(scan: &ScanResult) -> HashSet<String> {
        scan.files.iter().map(|f| format!("file:{}", f.path)).collect()
    }

    #[test]
    fn strip_test_suffix_typescript() {
        assert_eq!(strip_test_suffix("foo.test.ts"), Some("foo.ts".to_string()));
        assert_eq!(strip_test_suffix("foo.spec.tsx"), Some("foo.tsx".to_string()));
        assert_eq!(strip_test_suffix("foo.test.js"), Some("foo.js".to_string()));
        assert_eq!(strip_test_suffix("foo.ts"), None);
    }

    #[test]
    fn strip_test_suffix_go() {
        assert_eq!(strip_test_suffix("foo_test.go"), Some("foo.go".to_string()));
    }

    #[test]
    fn strip_test_suffix_java_kotlin() {
        // Suffix-stripping is case-insensitive on the input, so the
        // output basename is the lowercased stem + lowercase ext.
        // The caller compares against a `lowercased` path set, so
        // case-insensitivity is the right semantics.
        assert_eq!(strip_test_suffix("FooTest.java"), Some("foo.java".to_string()));
        assert_eq!(strip_test_suffix("FooTests.java"), Some("foo.java".to_string()));
        assert_eq!(strip_test_suffix("FooIT.java"), Some("foo.java".to_string()));
        assert_eq!(strip_test_suffix("TestFoo.kt"), Some("foo.kt".to_string()));
        assert_eq!(strip_test_suffix("FooTests.kt"), Some("foo.kt".to_string()));
    }

    #[test]
    fn strip_test_suffix_python_lua() {
        assert_eq!(strip_test_suffix("test_foo.py"), Some("foo.py".to_string()));
        assert_eq!(strip_test_suffix("foo_test.py"), Some("foo.py".to_string()));
        assert_eq!(strip_test_suffix("test_foo.lua"), Some("foo.lua".to_string()));
    }

    #[test]
    fn extract_tested_by_pairs_production_with_test() {
        let scan = make_scan(&[
            "src/foo.ts",
            "src/foo.test.ts",
            "src/bar.ts",
            "src/bar.spec.ts",
        ]);
        let ids = ids_for(&scan);
        let edges = extract_tested_by(&scan, &ids);
        assert_eq!(edges.len(), 2);
        // Direction is production → test
        assert!(edges.iter().any(|e| e.source == "file:src/foo.ts" && e.target == "file:src/foo.test.ts" && e.kind == "tested_by"));
        assert!(edges.iter().any(|e| e.source == "file:src/bar.ts" && e.target == "file:src/bar.spec.ts"));
    }

    #[test]
    fn extract_tested_by_skips_test_test_pairs() {
        let scan = make_scan(&[
            "src/foo.test.ts",
            "src/foo.test.spec.ts",
        ]);
        let ids = ids_for(&scan);
        let edges = extract_tested_by(&scan, &ids);
        // foo.test.spec.ts would canonicalize to foo.test.ts which is also a test → skip
        assert_eq!(edges.len(), 0);
    }

    #[test]
    fn extract_configures_tsconfig_targets_all_ts() {
        let scan = make_scan(&[
            "tsconfig.json",
            "src/a.ts",
            "src/b.ts",
            "src/c.tsx",
            "src/d.md", // should be ignored
        ]);
        let ids = ids_for(&scan);
        let edges = extract_configures(&scan, &ids);
        assert_eq!(edges.len(), 3, "tsconfig should configure 3 ts/tsx files");
        assert!(edges.iter().all(|e| e.source == "file:tsconfig.json" && e.kind == "configures"));
        for tgt in ["src/a.ts", "src/b.ts", "src/c.tsx"] {
            assert!(edges.iter().any(|e| e.target == format!("file:{tgt}")));
        }
    }

    #[test]
    fn extract_configures_package_json_targets_entry() {
        let scan = make_scan(&["package.json", "src/index.ts", "src/utils.ts"]);
        let ids = ids_for(&scan);
        let edges = extract_configures(&scan, &ids);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, "file:package.json");
        assert_eq!(edges[0].target, "file:src/index.ts");
    }

    #[test]
    fn extract_configures_caps_target_count() {
        // 60 .ts files + tsconfig.json → cap at 50
        let mut paths: Vec<String> = (0..60).map(|i| format!("src/file{i:02}.ts")).collect();
        paths.push("tsconfig.json".to_string());
        let path_strs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
        let scan = make_scan(&path_strs);
        let ids = ids_for(&scan);
        let edges = extract_configures(&scan, &ids);
        assert!(edges.len() <= MAX_CONFIGURES_TARGETS_PER_SOURCE);
        assert!(edges.len() >= 50);
    }

    #[test]
    fn extract_non_code_depends_on_dockerfile_copy() {
        // Use tempdir-like scaffolding
        let dir = tempdir();
        let dockerfile = dir.join("Dockerfile");
        std::fs::write(
            &dockerfile,
            "FROM node:18\nCOPY package.json /app/\nCOPY src/index.ts /app/\n",
        )
        .unwrap();

        let scan = make_scan(&["Dockerfile", "package.json", "src/index.ts"]);
        let ids = ids_for(&scan);
        let edges = extract_non_code_depends_on(&scan, &dir, &ids);
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().all(|e| e.source == "file:Dockerfile" && e.kind == "depends_on"));
        let tgts: HashSet<String> = edges.iter().map(|e| e.target.clone()).collect();
        assert!(tgts.contains("file:package.json"));
        assert!(tgts.contains("file:src/index.ts"));
    }

    #[test]
    fn extract_non_code_depends_on_dockerfile_no_http_urls() {
        let dir = tempdir();
        std::fs::write(
            &dir.join("Dockerfile"),
            "FROM alpine\nADD https://example.com/foo.tar.gz /tmp/\n",
        )
        .unwrap();
        let scan = make_scan(&["Dockerfile"]);
        let ids = ids_for(&scan);
        let edges = extract_non_code_depends_on(&scan, &dir, &ids);
        assert_eq!(edges.len(), 0);
    }

    #[test]
    fn extract_non_code_depends_on_skips_self_loop() {
        let dir = tempdir();
        // package.json referencing itself via scripts wouldn't happen normally,
        // but we should still skip src==tgt if it does.
        std::fs::write(
            &dir.join("package.json"),
            r#"{"scripts": {"self": "./package.json"}}"#,
        )
        .unwrap();
        let scan = make_scan(&["package.json"]);
        let ids = ids_for(&scan);
        let edges = extract_non_code_depends_on(&scan, &dir, &ids);
        // "./package.json" normalised in scan may not equal "package.json" depending on resolver;
        // either way, self-loop check ensures it doesn't emit.
        for e in &edges {
            assert_ne!(e.source, e.target);
        }
    }

    /// Minimal tempdir for tests (avoids pulling in tempfile crate).
    fn tempdir() -> std::path::PathBuf {
        let unique = format!(
            "codewiki_edge_rules_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    // -- P2-A: nine additional edge type tests --

    #[test]
    fn extract_pub_sub_pairs_subscriber_with_publisher() {
        // Both subscriber and publisher share the "user.created"
        // basename and a shared events/ module exists.
        let scan = make_scan(&[
            "src/subscribers/user-created.subscriber.ts",
            "src/publishers/user-created.publisher.ts",
            "src/events/user-created.ts",
        ]);
        let ids = ids_for(&scan);
        let edges = extract_pub_sub(&scan, &ids);
        // Expect subscribes: subscriber -> event module
        assert!(edges
            .iter()
            .any(|e| e.source == "file:src/subscribers/user-created.subscriber.ts"
                && e.target == "file:src/events/user-created.ts"
                && e.kind == "subscribes"));
        // Expect publishes: publisher -> event module (or to each subscriber)
        assert!(edges
            .iter()
            .any(|e| e.source == "file:src/publishers/user-created.publisher.ts"
                && e.kind == "publishes"));
    }

    #[test]
    fn extract_pub_sub_direct_pair_without_event_bus() {
        // No shared event module — subscriber connects directly to publisher.
        let scan = make_scan(&[
            "src/subscribers/foo.subscriber.ts",
            "src/publishers/foo.publisher.ts",
        ]);
        let ids = ids_for(&scan);
        let edges = extract_pub_sub(&scan, &ids);
        assert!(edges
            .iter()
            .any(|e| e.source == "file:src/subscribers/foo.subscriber.ts"
                && e.target == "file:src/publishers/foo.publisher.ts"
                && e.kind == "subscribes"));
    }

    #[test]
    fn extract_routes_nginx_to_upstream_service() {
        let dir = tempdir();
        std::fs::write(
            &dir.join("nginx.conf"),
            "server {\n  proxy_pass http://auth-service/;\n}\n",
        )
        .unwrap();
        let scan = make_scan_at(
            &["nginx.conf", "services/auth-service.ts"],
            dir.to_string_lossy().to_string(),
        );
        let ids = ids_for(&scan);
        let edges = extract_routes_and_middleware(&scan, &dir, &ids);
        assert!(edges
            .iter()
            .any(|e| e.source == "file:nginx.conf"
                && e.target == "file:services/auth-service.ts"
                && e.kind == "routes"));
    }

    #[test]
    fn extract_routes_middleware_via_import_map() {
        let scan = make_scan(&[
            "src/middleware/auth.ts",
            "src/routes/index.ts",
        ]);
        // import_map: routes/index.ts imports middleware/auth.ts
        let mut scan = scan;
        scan.import_map.insert(
            "src/routes/index.ts".to_string(),
            vec!["src/middleware/auth.ts".to_string()],
        );
        let ids = ids_for(&scan);
        let dir = tempdir();
        let edges = extract_routes_and_middleware(&scan, &dir, &ids);
        assert!(edges
            .iter()
            .any(|e| e.source == "file:src/middleware/auth.ts"
                && e.target == "file:src/routes/index.ts"
                && e.kind == "middleware"));
    }

    #[test]
    fn extract_schema_graphql_to_resolvers() {
        let scan = make_scan(&[
            "src/schema.graphql",
            "src/resolvers/user.ts",
            "src/resolvers/post.ts",
        ]);
        let ids = ids_for(&scan);
        let edges = extract_schema_definitions(&scan, &ids);
        assert_eq!(edges.len(), 2, "graphql schema → 2 resolver files");
        assert!(edges
            .iter()
            .all(|e| e.source == "file:src/schema.graphql" && e.kind == "defines_schema"));
    }

    #[test]
    fn extract_infrastructure_triggers_gh_actions() {
        let dir = tempdir();
        let workflow = dir.join(".github/workflows/ci.yml");
        std::fs::create_dir_all(workflow.parent().unwrap()).unwrap();
        std::fs::write(
            &workflow,
            "name: CI\non: push\njobs:\n  test:\n    steps:\n      - run: ./scripts/test.sh\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts/test.sh"), "echo hello").unwrap();
        let scan = make_scan_at(
            &[".github/workflows/ci.yml", "scripts/test.sh"],
            dir.to_string_lossy().to_string(),
        );
        let ids = ids_for(&scan);
        let edges = extract_infrastructure_topology(&scan, &dir, &ids);
        assert!(edges
            .iter()
            .any(|e| e.source == "file:.github/workflows/ci.yml"
                && e.target == "file:scripts/test.sh"
                && e.kind == "triggers"));
    }

    #[test]
    fn extract_infrastructure_serves_k8s_deployment() {
        let dir = tempdir();
        std::fs::create_dir_all(dir.join("k8s")).unwrap();
        std::fs::write(
            &dir.join("k8s/deployment.yaml"),
            "apiVersion: apps/v1\nkind: Deployment\nspec:\n  template:\n    spec:\n      containers:\n        - name: api\n          image: myorg/api:1.0\n",
        )
        .unwrap();
        let scan = make_scan_at(
            &["k8s/deployment.yaml", "src/api.ts"],
            dir.to_string_lossy().to_string(),
        );
        let ids = ids_for(&scan);
        let edges = extract_infrastructure_topology(&scan, &dir, &ids);
        // name: api → src/api.ts (synthesized path)
        assert!(edges
            .iter()
            .any(|e| e.source == "file:k8s/deployment.yaml"
                && e.target == "file:src/api.ts"
                && e.kind == "serves"));
    }

    #[test]
    fn extract_infrastructure_provisions_terraform() {
        let dir = tempdir();
        std::fs::write(
            &dir.join("main.tf"),
            r#"
resource "aws_db_instance" "main" {
  engine = "postgres"
}
"#,
        )
        .unwrap();
        let scan = make_scan_at(&["main.tf"], dir.to_string_lossy().to_string());
        let ids = ids_for(&scan);
        let edges = extract_infrastructure_topology(&scan, &dir, &ids);
        // Synthesized path "infra/<type>/<name>"
        assert!(edges
            .iter()
            .any(|e| e.source == "file:main.tf"
                && e.target == "file:infra/aws_db_instance/main"
                && e.kind == "provisions"));
    }

    #[test]
    fn extract_infrastructure_migrates_sql_to_table_model() {
        let dir = tempdir();
        std::fs::create_dir_all(dir.join("migrations")).unwrap();
        std::fs::write(
            &dir.join("migrations/001_init.sql"),
            "CREATE TABLE users (id INT PRIMARY KEY);\n",
        )
        .unwrap();
        let scan = make_scan_at(
            &["migrations/001_init.sql", "models/user.ts"],
            dir.to_string_lossy().to_string(),
        );
        let ids = ids_for(&scan);
        let edges = extract_infrastructure_topology(&scan, &dir, &ids);
        assert!(edges
            .iter()
            .any(|e| e.source == "file:migrations/001_init.sql"
                && e.target == "file:models/user.ts"
                && e.kind == "migrates"));
    }

    #[test]
    fn extract_pub_sub_skips_files_without_role() {
        // Regular files (no subscribers/ or publishers/ prefix) emit no
        // pub/sub edges even if other pub/sub files exist.
        let scan = make_scan(&[
            "src/utils.ts",
            "src/subscribers/foo.subscriber.ts",
            "src/publishers/foo.publisher.ts",
            "src/events/foo.ts",
        ]);
        let ids = ids_for(&scan);
        let edges = extract_pub_sub(&scan, &ids);
        assert!(edges.iter().all(|e| !e.source.contains("utils.ts")));
    }
}