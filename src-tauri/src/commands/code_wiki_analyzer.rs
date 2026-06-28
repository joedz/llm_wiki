// Phase 2 — ANALYZE. Per-batch LLM call that enriches file-level
// graph nodes with LLM-written `summary`, `tags`, and `complexity`.
// Mirrors UA's `file-analyzer.md` agent but kept narrow: we
// only enrich file nodes (no sub-file function/class extraction
// — that's left to M3 / Phase 2-b).
//
// Workflow per batch:
//   1. Build the user message: a JSON object with each file's
//      path, language, fileCategory, sizeLines, and (for `code`
//      files) the actual source content.
//   2. POST to the LLM with the system + user message.
//   3. Parse the response as JSON. Validate the schema strictly.
//   4. On failure, retry ONCE with a stricter prompt.
//   5. Apply the enrichments to the in-memory graph.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::code_wiki_batcher::BatchEntry;
use crate::commands::code_wiki_scanner::{ScannedFile, ScanResult};
use crate::llm_client::{call_llm, LlmProvider, LlmRequest, LlmResponse};

const MAX_SOURCE_BYTES_PER_FILE: usize = 32 * 1024;

/// Per-file enrichment produced by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEnrichment {
    pub path: String,
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub complexity: String,
}

/// The LLM's response shape. We keep it narrow for M2: just a
/// list of file-level enrichments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrichmentResponse {
    pub enrichments: Vec<FileEnrichment>,
}

/// One batch's worth of source content, prepared for the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSourceFile {
    pub path: String,
    pub language: String,
    pub file_category: String,
    pub size_lines: u32,
    /// File content (UTF-8). Truncated to MAX_SOURCE_BYTES_PER_FILE
    /// to bound the prompt size.
    pub content: String,
}

/// Build the user message for a batch: a JSON object describing
/// each file plus its source content.
pub fn build_batch_user_message(
    batch: &BatchEntry,
    project_root: &Path,
    files_by_path: &std::collections::HashMap<String, &ScannedFile>,
) -> Result<String, String> {
    let mut batch_files: Vec<BatchSourceFile> = Vec::new();
    for file_path in &batch.files {
        let scan = match files_by_path.get(file_path) {
            Some(s) => *s,
            None => continue,
        };
        let abs = project_root.join(file_path);
        let content = read_source_for_prompt(&abs);
        batch_files.push(BatchSourceFile {
            path: scan.path.clone(),
            language: scan.language.clone(),
            file_category: scan.file_category.clone(),
            size_lines: scan.size_lines,
            content,
        });
    }
    let payload = serde_json::json!({
        "category": batch.category,
        "files": batch_files,
    });
    serde_json::to_string_pretty(&payload)
        .map_err(|e| format!("serialize batch user message: {e}"))
}

fn read_source_for_prompt(path: &Path) -> String {
    let Ok(bytes) = fs::read(path) else { return String::new() };
    if bytes.len() > MAX_SOURCE_BYTES_PER_FILE {
        // Truncate with a marker so the LLM knows.
        let mut s = String::from_utf8_lossy(&bytes[..MAX_SOURCE_BYTES_PER_FILE]).to_string();
        s.push_str("\n\n/* ... truncated for prompt size ... */\n");
        return s;
    }
    String::from_utf8_lossy(&bytes).to_string()
}

const SYSTEM_PROMPT: &str = "You are an expert code analyst. Your job is to read source files and produce precise, structured knowledge-graph enrichments.\n\nFor every file in the input, you produce one enrichment object with:\n  - path: the file's path (string, exact match)\n  - summary: 1-2 sentences describing the file's purpose and role in the project. Be specific — name the functions/classes/patterns that matter, not the obvious category. Avoid generic phrases like \"contains utility functions\".\n  - tags: 2-5 short lowercase tags. Prefer specific domain terms (e.g. \"api-handler\", \"config-loader\", \"diff-algorithm\") over generic ones (\"code\").\n  - complexity: \"simple\" (under ~50 non-empty lines, single concern), \"moderate\" (typical), or \"complex\" (large file or multiple intertwined concerns).\n\nReturn your answer as JSON matching this schema:\n{\n  \"enrichments\": [\n    { \"path\": \"...\", \"summary\": \"...\", \"tags\": [\"...\"], \"complexity\": \"simple|moderate|complex\" }\n  ]\n}\n\nConstraints:\n- Emit exactly one enrichment per file in the input.\n- Do not add new fields.\n- Do not invent files.\n- Use English unless the file's content is clearly in another language (e.g. README in Chinese → summary in Chinese).";

fn user_message_with_schema_repeat(user: &str) -> String {
    // The repeated schema is a defensive measure: for short models
    // (or when the system message gets truncated by the provider),
    // repeating the schema at the end of the user message keeps the
    // LLM aligned.
    format!(
        "{user}\n\n---\nRespond with valid JSON matching this exact schema:\n\
         {{\"enrichments\": [{{\"path\": \"<file path>\", \"summary\": \"<one-line summary>\", \
         \"tags\": [\"<short tag>\"], \"complexity\": \"simple|moderate|complex\"}}]}}"
    )
}

/// Run Phase 2 for a single batch: build the prompt, call the
/// LLM, parse + validate the response. Retries once on schema
/// failure with a stricter prompt.
pub async fn analyze_batch(
    batch: &BatchEntry,
    project_root: &Path,
    files_by_path: &std::collections::HashMap<String, &ScannedFile>,
    llm_request: LlmRequest,
) -> Result<EnrichmentResponse, String> {
    let user = build_batch_user_message(batch, project_root, files_by_path)?;
    let mut last_err: Option<String> = None;
    for attempt in 0..2u32 {
        let user_for_attempt = if attempt == 0 {
            user.clone()
        } else {
            user_message_with_schema_repeat(&user)
        };
        let mut req = llm_request.clone();
        req.user = user_for_attempt;
        if attempt == 1 {
            // Second attempt: lower temperature for stricter output.
            req.temperature = 0.0;
        }
        let resp = call_llm(req, 2).await.map_err(|e| {
            format!(
                "LLM call failed (kind={}, status={:?}): {}",
                e.kind, e.status, e.message
            )
        })?;
        match parse_and_validate_enrichments(&resp.content, batch) {
            Ok(enr) => return Ok(enr),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| "unknown enrichment parse failure".to_string()))
}

fn parse_and_validate_enrichments(
    content: &str,
    batch: &BatchEntry,
) -> Result<EnrichmentResponse, String> {
    // Some models wrap the JSON in a markdown code fence. Strip it.
    let trimmed = content.trim();
    let json_str = if trimmed.starts_with("```") {
        // Find the closing fence.
        if let Some(end) = trimmed.rfind("```") {
            // Skip the opening ``` (and any language hint) up to
            // the first newline, take everything up to the
            // closing fence.
            let after_open = trimmed.find('\n').map(|i| i + 1).unwrap_or(3);
            let body = &trimmed[after_open..end];
            body.trim()
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    let parsed: Value = serde_json::from_str(json_str)
        .map_err(|e| format!("response is not valid JSON: {e}"))?;
    let obj = parsed
        .as_object()
        .ok_or_else(|| "response is not a JSON object".to_string())?;
    let arr = obj
        .get("enrichments")
        .ok_or_else(|| "response missing 'enrichments' array".to_string())?
        .as_array()
        .ok_or_else(|| "'enrichments' is not an array".to_string())?;
    let mut enrichments: Vec<FileEnrichment> = Vec::with_capacity(arr.len());
    let expected_paths: HashSet<&str> = batch.files.iter().map(|s| s.as_str()).collect();
    let mut seen_paths: HashSet<String> = HashSet::new();
    for (i, e) in arr.iter().enumerate() {
        let enr: FileEnrichment = serde_json::from_value(e.clone())
            .map_err(|e| format!("enrichment[{i}] shape invalid: {e}"))?;
        if !expected_paths.contains(enr.path.as_str()) {
            return Err(format!(
                "enrichment[{i}] path '{}' not in this batch",
                enr.path
            ));
        }
        if !seen_paths.insert(enr.path.clone()) {
            return Err(format!(
                "enrichment[{i}] path '{}' appears twice",
                enr.path
            ));
        }
        if !matches!(enr.complexity.as_str(), "simple" | "moderate" | "complex") {
            return Err(format!(
                "enrichment[{i}] complexity '{}' is not one of simple|moderate|complex",
                enr.complexity
            ));
        }
        enrichments.push(enr);
    }
    if enrichments.len() != batch.files.len() {
        return Err(format!(
            "got {} enrichments for batch with {} files",
            enrichments.len(),
            batch.files.len()
        ));
    }
    Ok(EnrichmentResponse { enrichments })
}

/// Heuristic token estimate for a batch: 4 chars per token,
/// batch user message length + system prompt.
pub fn estimate_batch_tokens(batch: &BatchEntry, project_root: &Path) -> u32 {
    let user_chars = build_batch_user_message(batch, project_root, &std::collections::HashMap::new())
        .map(|s| s.len() as u32)
        .unwrap_or(0);
    let system_chars = SYSTEM_PROMPT.len() as u32;
    ((user_chars + system_chars) / 4) + 256 // +256 for the LLM's reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::code_wiki_batcher::BatchEntry;
    use std::collections::HashMap;

    fn f(path: &str, lines: u32) -> ScannedFile {
        ScannedFile {
            path: path.to_string(),
            language: "rust".to_string(),
            size_lines: lines,
            file_category: "code".to_string(),
        }
    }

    fn sample_batch() -> BatchEntry {
        BatchEntry {
            batch_index: 0,
            category: "code".to_string(),
            files: vec!["src/lib.rs".to_string(), "src/main.rs".to_string()],
        }
    }

    fn sample_files() -> HashMap<String, ScannedFile> {
        let mut m = HashMap::new();
        m.insert("src/lib.rs".to_string(), f("src/lib.rs", 10));
        m.insert("src/main.rs".to_string(), f("src/main.rs", 20));
        m
    }

    #[test]
    fn parse_and_validate_accepts_well_formed_response() {
        let batch = sample_batch();
        let response = r#"{
            "enrichments": [
                {"path": "src/lib.rs", "summary": "Tiny log lib.", "tags": ["logging"], "complexity": "simple"},
                {"path": "src/main.rs", "summary": "Demo main.", "tags": ["demo"], "complexity": "simple"}
            ]
        }"#;
        let out = parse_and_validate_enrichments(response, &batch).expect("parse");
        assert_eq!(out.enrichments.len(), 2);
        assert_eq!(out.enrichments[0].summary, "Tiny log lib.");
    }

    #[test]
    fn parse_and_validate_strips_markdown_code_fence() {
        let batch = sample_batch();
        let response = r#"```json
{
  "enrichments": [
    {"path": "src/lib.rs", "summary": "x", "tags": [], "complexity": "moderate"},
    {"path": "src/main.rs", "summary": "y", "tags": [], "complexity": "moderate"}
  ]
}
```"#;
        let out = parse_and_validate_enrichments(response, &batch).expect("parse");
        assert_eq!(out.enrichments.len(), 2);
    }

    #[test]
    fn parse_and_validate_rejects_missing_paths() {
        let batch = sample_batch();
        let response = r#"{
            "enrichments": [
                {"path": "src/lib.rs", "summary": "x", "tags": [], "complexity": "moderate"}
            ]
        }"#;
        let err = parse_and_validate_enrichments(response, &batch).unwrap_err();
        assert!(err.contains("1 enrichments"), "got: {err}");
    }

    #[test]
    fn parse_and_validate_rejects_invalid_complexity() {
        let batch = sample_batch();
        let response = r#"{
            "enrichments": [
                {"path": "src/lib.rs", "summary": "x", "tags": [], "complexity": "extreme"},
                {"path": "src/main.rs", "summary": "y", "tags": [], "complexity": "moderate"}
            ]
        }"#;
        let err = parse_and_validate_enrichments(response, &batch).unwrap_err();
        assert!(err.contains("complexity"), "got: {err}");
    }

    #[test]
    fn parse_and_validate_rejects_path_outside_batch() {
        let batch = sample_batch();
        let response = r#"{
            "enrichments": [
                {"path": "src/lib.rs", "summary": "x", "tags": [], "complexity": "moderate"},
                {"path": "src/other.rs", "summary": "y", "tags": [], "complexity": "moderate"}
            ]
        }"#;
        let err = parse_and_validate_enrichments(response, &batch).unwrap_err();
        assert!(err.contains("not in this batch"), "got: {err}");
    }

    #[test]
    fn parse_and_validate_rejects_duplicate_path() {
        let batch = sample_batch();
        let response = r#"{
            "enrichments": [
                {"path": "src/lib.rs", "summary": "x", "tags": [], "complexity": "moderate"},
                {"path": "src/lib.rs", "summary": "y", "tags": [], "complexity": "moderate"}
            ]
        }"#;
        let err = parse_and_validate_enrichments(response, &batch).unwrap_err();
        assert!(err.contains("twice"), "got: {err}");
    }

    #[test]
    fn build_batch_user_message_includes_all_files() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/lib.rs"), "// lib\npub fn x() {}\n").unwrap();
        std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
        let batch = sample_batch();
        let owned = sample_files();
        let refs: HashMap<String, &ScannedFile> =
            owned.iter().map(|(k, v)| (k.clone(), v)).collect();
        let msg = build_batch_user_message(&batch, &project, &refs).expect("build");
        assert!(msg.contains("\"src/lib.rs\""), "missing lib.rs: {msg}");
        assert!(msg.contains("\"src/main.rs\""), "missing main.rs: {msg}");
        assert!(msg.contains("pub fn x"), "missing lib content: {msg}");
    }

    #[test]
    fn user_message_with_schema_repeat_appends_block() {
        let out = user_message_with_schema_repeat("hello");
        assert!(out.starts_with("hello"));
        assert!(out.contains("Respond with valid JSON"));
    }

    #[test]
    fn estimate_batch_tokens_is_positive() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/lib.rs"), "x".repeat(4000)).unwrap();
        let batch = sample_batch();
        let n = estimate_batch_tokens(&batch, &project);
        assert!(n > 100, "estimate too low: {n}");
    }
}
