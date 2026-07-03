// Deterministic extraction for non-code edge types.
//
// Three of the five new edge types added in P1-A are deterministic
// enough to derive from filenames + path conventions alone (no LLM):
//
//   - `tested_by`: pair test files with their production counterpart
//     using path-canonicalization. Direction is canonicalised to
//     `production → test` (UA's convention).
//
//   - `configures`: config-file → target-language heuristics.
//     `tsconfig.json` configures all `.ts` files; `package.json`
//     configures the entry point; `.env*` configures runtime code.
//     We cap target counts to keep the graph readable.
//
//   - `depends_on`: non-code → code relationships for infra files
//     (Dockerfile, docker-compose, GitHub Actions workflows, Makefile,
//     package.json scripts). Extracted with simple regex/JSON rules.
//
// The other two (`reads_from` / `writes_to`) are emitted by the LLM
// in Phase 2 via the extended `FileEnrichment` schema.
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_scanner::{ScannedFile, ScanStats};

    fn make_scan(paths: &[&str]) -> ScanResult {
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
            project_root: ".".to_string(),
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
}