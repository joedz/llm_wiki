// Phase 1 — SCAN. Mirrors UA's project-scanner agent's deterministic
// half (file enumeration + language + category + lines) plus the
// no-LLM narrative extraction (project name + description from
// README, frameworks from manifest files). When Phase 2 is wired in
// (M2), LLM calls layer on top to enrich with language-specific
// narratives. For M1 we produce the same shape without LLM so the
// downstream pipeline can be exercised end-to-end.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::commands::code_wiki::META_FILE;

/// Default directories to skip during enumeration. These are
/// matched by directory name (not path) so a top-level `node_modules`
/// inside a vendored dep is also skipped.
const DEFAULT_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".output",
    ".cache",
    "vendor", // Go's vendored deps
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    ".venv",
    "venv",
    "env",
    ".idea",
    ".vscode",
    ".gradle",
    ".idea_modules",
];

/// Canonical language id per file extension. Anything not in the
/// table falls back to `"unknown"` so the writer doesn't need a
/// second special case. Files without an extension are matched by
/// filename (e.g. `Dockerfile`, `Makefile`).
fn language_for(path: &Path) -> String {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let ext = ext.to_ascii_lowercase();
        let mapped = match ext.as_str() {
            "ts" | "tsx" | "cts" | "mts" => "typescript",
            "js" | "jsx" | "cjs" | "mjs" => "javascript",
            "py" | "pyi" | "pyw" => "python",
            "rs" => "rust",
            "go" => "go",
            "java" => "java",
            "kt" | "kts" => "kotlin",
            "rb" => "ruby",
            "php" => "php",
            "c" | "h" => "c",
            "cc" | "cpp" | "cxx" | "hpp" | "hxx" => "cpp",
            "cs" => "csharp",
            "swift" => "swift",
            "scala" | "sbt" => "scala",
            "m" | "mm" => "objc",
            "r" => "r",
            "lua" => "lua",
            "pl" | "pm" => "perl",
            "ex" | "exs" => "elixir",
            "clj" | "cljs" => "clojure",
            "hs" => "haskell",
            "ml" | "mli" => "ocaml",
            "dart" => "dart",
            "vue" => "vue",
            "svelte" => "svelte",
            "md" | "mdx" => "markdown",
            "rst" => "rst",
            "txt" => "text",
            "json" | "jsonc" | "json5" => "json",
            "yaml" | "yml" => "yaml",
            "toml" => "toml",
            "xml" => "xml",
            "html" | "htm" => "html",
            "css" | "scss" | "sass" | "less" => "css",
            "sql" => "sql",
            "graphql" | "gql" => "graphql",
            "proto" => "protobuf",
            "tf" | "tfvars" | "hcl" => "terraform",
            "dockerfile" => "dockerfile",
            "sh" | "bash" | "zsh" => "bash",
            "ps1" => "powershell",
            "bat" | "cmd" => "batch",
            "makefile" | "mk" => "makefile",
            _ => return ext,
        };
        return mapped.to_string();
    }
    // No extension — match by filename. Mirrors UA's lookup table.
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    match filename.as_str() {
        "dockerfile" => return "dockerfile".to_string(),
        "makefile" | "gnumakefile" => return "makefile".to_string(),
        "jenkinsfile" => return "groovy".to_string(), // UA maps to "groovy" too
        "procfile" => return "procfile".to_string(),
        "vagrantfile" => return "ruby".to_string(),
        ".gitignore" | ".gitattributes" | ".dockerignore" => return "gitignore".to_string(),
        _ => "unknown".to_string(),
    }
}

/// Map a relative file path to a UA `fileCategory`. Priority order
/// matches UA's table: dockerfile / compose / k8s / ci-cd are
/// `infra`, `.md` is `docs`, `*.tf` is `infra`, etc.
fn file_category_for(rel: &str, language: &str) -> String {
    let lower = rel.to_ascii_lowercase();
    let filename = lower.rsplit_once('/').map(|(_, f)| f).unwrap_or(lower.as_str());

    // LICENSE is special — UA treats it as `code` (an exception),
    // not docs. Match it before the generic markdown rule.
    if filename == "license" {
        return "code".to_string();
    }
    // Docker / compose / k8s / CI / Makefile / Procfile / Vagrantfile
    if matches!(
        filename,
        "dockerfile" | "dockerfile.prod" | "dockerfile.dev"
            | "compose.yml" | "compose.yaml" | "docker-compose.yml" | "docker-compose.yaml"
            | "makefile" | "gnumakefile"
            | "jenkinsfile"
            | "procfile"
            | "vagrantfile"
    ) {
        return "infra".to_string();
    }
    if filename == ".gitlab-ci.yml" || filename == ".dockerignore" {
        return "infra".to_string();
    }
    if lower.starts_with(".github/workflows/") || lower.starts_with(".circleci/") {
        return "infra".to_string();
    }
    if lower.starts_with("k8s/") || lower.starts_with("kubernetes/") {
        return "infra".to_string();
    }
    if lower.ends_with(".k8s.yml") || lower.ends_with(".k8s.yaml") {
        return "infra".to_string();
    }
    if language == "terraform" {
        return "infra".to_string();
    }
    // YAML / JSON / TOML / XML / env / config-y stuff
    if matches!(
        &*language,
        "yaml" | "json" | "toml" | "xml" | "ini" | "env" | "gradle"
    ) {
        return "config".to_string();
    }

    // Markdown / docs
    if language == "markdown" || language == "rst" {
        return "docs".to_string();
    }
    // Data — SQL DDL, GraphQL, Protobuf
    if language == "graphql" || language == "protobuf" {
        return "data".to_string();
    }
    if language == "sql" {
        return "data".to_string();
    }
    // HTML / CSS — markup
    if language == "html" || language == "css" {
        return "markup".to_string();
    }
    // Shell scripts at fileCategory = script (vs. shell inside code)
    if matches!(&*language, "bash" | "powershell" | "batch") {
        return "script".to_string();
    }
    "code".to_string()
}


fn count_lines(path: &Path) -> u32 {
    let Ok(file) = fs::File::open(path) else { return 0 };
    let mut reader = BufReader::new(file);
    let mut count = 0u32;
    let mut buf = String::new();
    loop {
        match reader.read_line(&mut buf) {
            Ok(0) => break,
            Ok(_) => count += 1,
            Err(_) => break,
        }
        buf.clear();
    }
    count
}

fn is_hidden_dir(name: &str) -> bool {
    name.starts_with('.') && name != "."
}

fn is_skip_dir(name: &str) -> bool {
    DEFAULT_SKIP_DIRS.contains(&name)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScannedFile {
    pub path: String,
    pub language: String,
    pub size_lines: u32,
    pub file_category: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScanStats {
    pub files_scanned: u32,
    pub by_category: BTreeMap<String, u32>,
    pub by_language: BTreeMap<String, u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ScanResult {
    pub project_root: String,
    pub files: Vec<ScannedFile>,
    pub total_files: u32,
    pub filtered_by_ignore: u32,
    pub estimated_complexity: String,
    pub stats: ScanStats,
    /// Project name from manifest, or repo directory name as fallback.
    pub project_name: String,
    /// Description from `description` field of the primary manifest, or
    /// first paragraph of README, or empty.
    pub project_description: String,
    /// Detected frameworks (UI libraries, test frameworks, web
    /// frameworks, infra tooling) — extracted deterministically from
    /// manifest dependency lists. LLM enrichment (Phase 1 narrative)
    /// layers on top later.
    pub frameworks: Vec<String>,
    /// Git HEAD at scan time, or empty if not a git repo.
    pub git_commit_hash: String,
}

// --- Phase 1 deterministic execution ------------------------------------

pub fn scan_project_inner(project_root: &Path) -> Result<ScanResult, String> {
    if !project_root.is_dir() {
        return Err(format!("project root {:?} is not a directory", project_root));
    }

    let mut files: Vec<ScannedFile> = Vec::new();
    let mut by_category: BTreeMap<String, u32> = BTreeMap::new();
    let mut by_language: BTreeMap<String, u32> = BTreeMap::new();

    for entry in WalkDir::new(project_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        // Skip files inside a directory the walker descended into
        // that matches the skip list. WalkDir visits each entry
        // individually so we check ancestors here.
        if rel.split('/').any(is_skip_dir) {
            continue;
        }
        if rel.split('/').any(is_hidden_dir) {
            continue;
        }

        let language = language_for(path);
        let size_lines = count_lines(path);
        let file_category = file_category_for(&rel, &language);

        *by_category.entry(file_category.clone()).or_insert(0) += 1;
        *by_language.entry(language.clone()).or_insert(0) += 1;
        files.push(ScannedFile { path: rel, language, size_lines, file_category });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    let total_files = files.len() as u32;
    let stats = ScanStats { files_scanned: total_files, by_category, by_language };
    let estimated_complexity = estimate_complexity(&stats);

    let project_name = detect_project_name(project_root, &files)
        .unwrap_or_else(|| project_root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
    let project_description = detect_project_description(project_root, &project_name);
    let frameworks = detect_frameworks(project_root, &files);
    let git_commit_hash = detect_git_head(project_root);

    Ok(ScanResult {
        project_root: project_root.to_string_lossy().to_string(),
        files,
        total_files,
        filtered_by_ignore: 0, // Phase 0.5 plugs .understandignore here
        estimated_complexity,
        stats,
        project_name,
        project_description,
        frameworks,
        git_commit_hash,
    })
}

fn estimate_complexity(stats: &ScanStats) -> String {
    let total: u32 = stats.by_category.values().sum();
    match total {
        0..=49 => "simple".to_string(),
        50..=499 => "moderate".to_string(),
        _ => "complex".to_string(),
    }
}

// --- Manifest detection ------------------------------------------------

/// Read a small file as UTF-8, swallowing IO errors. Used for
/// package.json / Cargo.toml / pyproject.toml / README snippets.
fn read_text_small(path: &Path) -> Option<String> {
    let f = fs::File::open(path).ok()?;
    let r = BufReader::new(f);
    let mut out = String::new();
    for (i, line) in r.lines().enumerate() {
        if i >= 1000 { break; }
        if let Ok(l) = line {
            out.push_str(&l);
            out.push('\n');
        }
    }
    Some(out)
}

fn find_first_existing(project_root: &Path, candidates: &[&str]) -> Option<PathBuf> {
    for c in candidates {
        let p = project_root.join(c);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Project name priority (matches UA):
///   1. `package.json` `name`
///   2. `Cargo.toml` `[package].name`
///   3. `pyproject.toml` `[project].name` or `[tool.poetry].name`
///   4. go.mod module path's last segment
///   5. directory name
fn detect_project_name(project_root: &Path, _files: &[ScannedFile]) -> Option<String> {
    if let Some(p) = find_first_existing(project_root, &["package.json"]) {
        if let Some(s) = read_text_small(&p) {
            if let Some(name) = json_string_field(&s, "name") {
                return Some(name);
            }
        }
    }
    if let Some(p) = find_first_existing(project_root, &["Cargo.toml"]) {
        if let Some(s) = read_text_small(&p) {
            if let Some(name) = toml_section_field(&s, "package", "name") {
                return Some(name);
            }
        }
    }
    if let Some(p) = find_first_existing(project_root, &["pyproject.toml"]) {
        if let Some(s) = read_text_small(&p) {
            if let Some(name) = toml_section_field(&s, "project", "name") {
                return Some(name);
            }
            if let Some(name) = toml_section_field(&s, "tool.poetry", "name") {
                return Some(name);
            }
        }
    }
    if let Some(p) = find_first_existing(project_root, &["go.mod"]) {
        if let Some(s) = read_text_small(&p) {
            for line in s.lines() {
                if let Some(rest) = line.strip_prefix("module ") {
                    let last = rest.trim().rsplit_once('/').map(|(_, n)| n.to_string()).unwrap_or_else(|| rest.trim().to_string());
                    if !last.is_empty() {
                        return Some(last);
                    }
                }
            }
        }
    }
    project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
}

fn json_string_field(json: &str, field: &str) -> Option<String> {
    let needle = format!("\"{}\"", field);
    let idx = json.find(&needle)?;
    let rest = &json[idx + needle.len()..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    if !after.starts_with('"') { return None; }
    let end = after[1..].find('"')?;
    Some(after[1..1 + end].to_string())
}

fn toml_section_field(toml: &str, section: &str, field: &str) -> Option<String> {
    // Find the section header `[section]`
    let header = format!("[{}]", section);
    let start = toml.find(&header)?;
    let after = &toml[start + header.len()..];
    // Find next `[` (end of section) or EOF
    let end = after.find("\n[").unwrap_or(after.len());
    let section_body = &after[..end];
    for line in section_body.lines() {
        let line = line.trim();
        if line.starts_with('#') { continue; }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == field {
                let v = v.trim().trim_matches('"');
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Description priority:
///   1. Manifest `description` field
///   2. README first paragraph (up to 300 chars)
fn detect_project_description(project_root: &Path, _project_name: &str) -> String {
    if let Some(p) = find_first_existing(project_root, &["package.json"]) {
        if let Some(s) = read_text_small(&p) {
            if let Some(d) = json_string_field(&s, "description") {
                if !d.is_empty() { return d; }
            }
        }
    }
    if let Some(p) = find_first_existing(project_root, &["Cargo.toml"]) {
        if let Some(s) = read_text_small(&p) {
            if let Some(d) = toml_section_field(&s, "package", "description") {
                if !d.is_empty() { return d; }
            }
        }
    }
    let readme = find_first_existing(
        project_root,
        &["README.md", "README.rst", "README", "readme.md"],
    );
    if let Some(p) = readme {
        if let Some(s) = read_text_small(&p) {
            // First non-empty, non-heading paragraph
            let mut in_para = false;
            let mut buf = String::new();
            for line in s.lines() {
                let line = line.trim();
                if line.is_empty() {
                    if in_para && !buf.is_empty() { break; }
                    continue;
                }
                if line.starts_with('#') {
                    if in_para && !buf.is_empty() { break; }
                    continue;
                }
                in_para = true;
                if !buf.is_empty() { buf.push(' '); }
                buf.push_str(line);
                if buf.len() > 300 { break; }
            }
            return buf;
        }
    }
    String::new()
}

// --- Frameworks detection ----------------------------------------------

/// Map a dependency name to a UA framework label. The function
/// intentionally does *not* invent labels — only well-known
/// ecosystems are recognised. Anything not in the table is
/// dropped (the LLM phase can add narrative frameworks later).
fn framework_label_for(dep: &str) -> Option<&'static str> {
    let d = dep.to_ascii_lowercase();
    let d = d.rsplit_once('/').map(|(_, n)| n).unwrap_or(d.as_str());
    let mapped = match d {
        // JS / TS frontend
        "react" | "@types/react" => Some("React"),
        "vue" => Some("Vue"),
        "svelte" => Some("Svelte"),
        "@angular/core" => Some("Angular"),
        "solid-js" => Some("Solid"),
        // JS / TS meta-frameworks
        "next" => Some("Next.js"),
        "nuxt" => Some("Nuxt"),
        "remix" | "@remix-run/react" => Some("Remix"),
        "sveltekit" => Some("SvelteKit"),
        "astro" => Some("Astro"),
        // JS / TS build / tooling
        "vite" => Some("Vite"),
        "webpack" => Some("Webpack"),
        "esbuild" => Some("esbuild"),
        "rollup" => Some("Rollup"),
        "parcel" => Some("Parcel"),
        "turbopack" => Some("Turbopack"),
        // JS / TS test
        "vitest" => Some("Vitest"),
        "jest" => Some("Jest"),
        "mocha" => Some("Mocha"),
        "playwright" => Some("Playwright"),
        "cypress" => Some("Cypress"),
        // JS / TS state
        "redux" | "@reduxjs/toolkit" => Some("Redux"),
        "zustand" => Some("Zustand"),
        "mobx" => Some("MobX"),
        "recoil" => Some("Recoil"),
        "jotai" => Some("Jotai"),
        // JS / TS styling
        "tailwindcss" => Some("TailwindCSS"),
        "styled-components" => Some("styled-components"),
        "emotion" => Some("Emotion"),
        // JS / TS server
        "express" => Some("Express"),
        "fastify" => Some("Fastify"),
        "koa" => Some("Koa"),
        "hapi" => Some("Hapi"),
        "nestjs" => Some("NestJS"),
        // JS / TS ORM
        "prisma" => Some("Prisma"),
        "typeorm" => Some("TypeORM"),
        "sequelize" => Some("Sequelize"),
        "mongoose" => Some("Mongoose"),
        // Desktop
        "tauri" | "@tauri-apps/api" | "@tauri-apps/cli" => Some("Tauri"),
        "electron" => Some("Electron"),
        // Rust web
        "actix-web" => Some("actix-web"),
        "actix-web-httpauth" => Some("actix-web"),
        "axum" => Some("Axum"),
        "rocket" => Some("Rocket"),
        "warp" => Some("Warp"),
        // Rust ORM
        "diesel" => Some("Diesel"),
        "sea-orm" => Some("SeaORM"),
        // Rust async
        "tokio" => Some("Tokio"),
        // Rust serialization
        "serde" | "serde_json" => Some("serde"),
        // Python web
        "django" => Some("Django"),
        "djangorestframework" => Some("Django REST"),
        "fastapi" => Some("FastAPI"),
        "flask" => Some("Flask"),
        "starlette" => Some("Starlette"),
        "uvicorn" => Some("Uvicorn"),
        "gunicorn" => Some("Gunicorn"),
        "tornado" => Some("Tornado"),
        "aiohttp" => Some("aiohttp"),
        // Python ORM
        "sqlalchemy" => Some("SQLAlchemy"),
        "alembic" => Some("Alembic"),
        "pydantic" => Some("Pydantic"),
        // Python task
        "celery" => Some("Celery"),
        // Python test
        "pytest" => Some("pytest"),
        "hypothesis" => Some("Hypothesis"),
        // Ruby
        "rails" | "railties" => Some("Ruby on Rails"),
        "sinatra" => Some("Sinatra"),
        "grape" => Some("Grape"),
        "rspec" => Some("RSpec"),
        "sidekiq" => Some("Sidekiq"),
        "devise" => Some("Devise"),
        "pundit" => Some("Pundit"),
        // Go web
        "github.com/gin-gonic/gin" => Some("Gin"),
        "github.com/labstack/echo" => Some("Echo"),
        "github.com/gofiber/fiber" => Some("Fiber"),
        "github.com/go-chi/chi" => Some("Chi"),
        // Go ORM
        "gorm.io/gorm" => Some("GORM"),
        // JVM
        "spring-boot" | "spring-web" | "spring-boot-starter-web" => Some("Spring Boot"),
        "quarkus" => Some("Quarkus"),
        "micronaut" => Some("Micronaut"),
        "hibernate" => Some("Hibernate"),
        "ktor" => Some("Ktor"),
        "junit-jupiter" => Some("JUnit"),
        // PHP
        "laravel/framework" => Some("Laravel"),
        "symfony/framework-bundle" => Some("Symfony"),
        _ => None,
    };
    mapped
}

fn push_unique(out: &mut Vec<String>, label: &str) {
    if !out.iter().any(|x| x == label) {
        out.push(label.to_string());
    }
}

fn detect_frameworks_from_package_json(json: &str) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(start) = json.find(&format!("\"{key}\"")) {
            let brace_start = json[start..].find('{').map(|i| start + i);
            if let Some(bs) = brace_start {
                let mut depth = 0;
                let mut end = bs;
                for (i, c) in json[bs..].char_indices() {
                    if c == '{' { depth += 1; }
                    else if c == '}' { depth -= 1; if depth == 0 { end = bs + i; break; } }
                }
                let body = &json[bs + 1..end];
                // Iterate over each "name": "version" entry
                let mut i = 0;
                while i < body.len() {
                    if let Some(q1) = body[i..].find('"') {
                        let after_q1 = i + q1 + 1;
                        if let Some(q2) = body[after_q1..].find('"') {
                            let name = &body[after_q1..after_q1 + q2];
                            if let Some(label) = framework_label_for(name) {
                                push_unique(&mut out, label);
                            }
                            i = after_q1 + q2 + 1;
                            continue;
                        }
                    }
                    break;
                }
            }
        }
    }
    out
}

fn detect_frameworks_from_cargo_toml(toml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let section = if let Some(s) = toml.find("[dependencies]") {
        toml[s..].split("\n[").next().unwrap_or("")
    } else if let Some(s) = toml.find("[dev-dependencies]") {
        toml[s..].split("\n[").next().unwrap_or("")
    } else {
        return out;
    };
    for line in section.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') { continue; }
        let name = line.split(['=', ' ']).next().unwrap_or("").trim();
        if let Some(label) = framework_label_for(name) {
            push_unique(&mut out, label);
        }
    }
    out
}

fn detect_frameworks_from_pyproject(toml: &str) -> Vec<String> {
    let mut out = Vec::new();
    for section_name in ["project", "tool.poetry.dependencies", "dependency-groups"] {
        if let Some(s) = toml.find(&format!("[{section_name}]")) {
            let body = toml[s..].split("\n[").next().unwrap_or("");
            for line in body.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with('[') { continue; }
                let name = line.split(['=', ' ']).next().unwrap_or("").trim();
                if let Some(label) = framework_label_for(name) {
                    push_unique(&mut out, label);
                }
            }
        }
    }
    out
}

fn detect_frameworks_from_go_mod(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("require (") {
            in_block = true;
            continue;
        }
        if in_block && line == ")" {
            in_block = false;
            continue;
        }
        if in_block || line.starts_with("require ") {
            let name = line.split_whitespace().nth(1).unwrap_or("");
            if let Some(label) = framework_label_for(name) {
                push_unique(&mut out, label);
            }
        }
    }
    out
}

fn detect_frameworks(project_root: &Path, _files: &[ScannedFile]) -> Vec<String> {
    let mut out = Vec::new();
    // Order matters: stop at the first manifest that gives a
    // reasonable answer. Multi-language projects get one
    // representative manifest per language; we dedupe labels so
    // "React" doesn't appear twice.
    for (filename, parser) in [
        ("package.json", detect_frameworks_from_package_json as fn(&str) -> Vec<String>),
        ("Cargo.toml", detect_frameworks_from_cargo_toml),
        ("pyproject.toml", detect_frameworks_from_pyproject),
        ("go.mod", detect_frameworks_from_go_mod),
    ] {
        let path = project_root.join(filename);
        if let Some(s) = read_text_small(&path) {
            for label in parser(&s) {
                push_unique(&mut out, &label);
            }
        }
    }
    // Infra tooling from file presence — mirrors UA's heuristic.
    for rel in ["Dockerfile", "docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"] {
        if project_root.join(rel).is_file() {
            push_unique(&mut out, "Docker");
            break;
        }
    }
    for entry in WalkDir::new(project_root).max_depth(1).into_iter().flatten() {
        if entry.file_name() == "Dockerfile" {
            push_unique(&mut out, "Docker");
            break;
        }
    }
    if project_root.join(".github/workflows").is_dir() {
        push_unique(&mut out, "GitHub Actions");
    }
    if project_root.join(".gitlab-ci.yml").is_file() {
        push_unique(&mut out, "GitLab CI");
    }
    if project_root.join("Jenkinsfile").is_file() {
        push_unique(&mut out, "Jenkins");
    }
    for entry in WalkDir::new(project_root).max_depth(1).into_iter().flatten() {
        if entry.path().extension().and_then(|s| s.to_str()) == Some("tf") {
            push_unique(&mut out, "Terraform");
            break;
        }
    }
    out
}

fn detect_git_head(project_root: &Path) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .arg("rev-parse")
        .arg("HEAD")
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // Walkdir will produce results in a deterministic order for our test
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/some-pkg")).unwrap();
        std::fs::create_dir_all(dir.path().join(".git/refs")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{ \"name\": \"demo\", \"dependencies\": { \"react\": \"^18.0.0\", \"vite\": \"^5.0.0\" } }").unwrap();
        std::fs::write(dir.path().join("src/main.ts"), "const a = 1\nconst b = 2\n").unwrap();
        std::fs::write(dir.path().join("src/main.test.ts"), "test()\n").unwrap();
        std::fs::write(dir.path().join("README.md"), "# Demo\n\nA demo project for the scanner test.\n").unwrap();
        std::fs::write(dir.path().join("node_modules/some-pkg/index.js"), "ignore\n").unwrap();
        std::fs::write(dir.path().join(".git/refs/HEAD"), "ignore\n").unwrap();
        std::fs::write(dir.path().join("Dockerfile"), "FROM scratch\n").unwrap();
        dir
    }

    #[test]
    fn scan_skips_node_modules_and_hidden_dirs() {
        let dir = fixture();
        let result = scan_project_inner(dir.path()).expect("scan");
        let paths: Vec<&str> = result.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"package.json"));
        assert!(paths.contains(&"src/main.ts"));
        assert!(paths.contains(&"src/main.test.ts"));
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&"Dockerfile"));
        assert!(!paths.iter().any(|p| p.contains("node_modules")));
        assert!(!paths.iter().any(|p| p.starts_with(".git")));
    }

    #[test]
    fn scan_detects_languages_and_categories() {
        let dir = fixture();
        let result = scan_project_inner(dir.path()).expect("scan");
        let f = |p: &str| result.files.iter().find(|x| x.path == p).expect(p);
        assert_eq!(f("src/main.ts").language, "typescript");
        assert_eq!(f("src/main.ts").file_category, "code");
        assert_eq!(f("package.json").file_category, "config");
        assert_eq!(f("README.md").file_category, "docs");
        assert_eq!(f("Dockerfile").file_category, "infra");
    }

    #[test]
    fn scan_detects_frameworks_from_package_json() {
        let dir = fixture();
        let result = scan_project_inner(dir.path()).expect("scan");
        assert!(result.frameworks.contains(&"React".to_string()));
        assert!(result.frameworks.contains(&"Vite".to_string()));
    }

    #[test]
    fn scan_detects_project_name_and_description() {
        let dir = fixture();
        let result = scan_project_inner(dir.path()).expect("scan");
        assert_eq!(result.project_name, "demo");
        assert!(result.project_description.contains("demo project"));
    }

    #[test]
    fn scan_includes_docker_and_github_actions_in_frameworks() {
        let dir = fixture();
        std::fs::create_dir_all(dir.path().join(".github/workflows")).unwrap();
        std::fs::write(dir.path().join(".github/workflows/ci.yml"), "name: ci\n").unwrap();
        let result = scan_project_inner(dir.path()).expect("scan");
        assert!(result.frameworks.contains(&"Docker".to_string()));
        assert!(result.frameworks.contains(&"GitHub Actions".to_string()));
    }

    #[test]
    fn language_for_handles_known_and_unknown_extensions() {
        assert_eq!(language_for(Path::new("foo.ts")), "typescript");
        assert_eq!(language_for(Path::new("foo.rs")), "rust");
        assert_eq!(language_for(Path::new("foo.py")), "python");
        assert_eq!(language_for(Path::new("foo.unknown_ext")), "unknown_ext");
        assert_eq!(language_for(Path::new("Dockerfile")), "dockerfile");
    }

    #[test]
    fn file_category_license_is_code() {
        assert_eq!(file_category_for("LICENSE", "unknown"), "code");
    }

    #[test]
    fn json_string_field_parses() {
        let s = r#"{"name": "myapp", "description": "hi"}"#;
        assert_eq!(json_string_field(s, "name"), Some("myapp".to_string()));
        assert_eq!(json_string_field(s, "description"), Some("hi".to_string()));
        assert_eq!(json_string_field(s, "missing"), None);
    }

    #[test]
    fn toml_section_field_parses() {
        let s = "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n";
        assert_eq!(toml_section_field(s, "package", "name"), Some("my-crate".to_string()));
        assert_eq!(toml_section_field(s, "package", "version"), Some("0.1.0".to_string()));
        assert_eq!(toml_section_field(s, "missing", "name"), None);
    }

    #[test]
    fn frameworks_parses_rust_dependencies() {
        let s = "[dependencies]\nserde = \"1\"\nserde_json = \"1\"\ntokio = { version = \"1\" }\n";
        let f = detect_frameworks_from_cargo_toml(s);
        assert!(f.contains(&"serde".to_string()));
        assert!(f.contains(&"Tokio".to_string()));
    }
}
