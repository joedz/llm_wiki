// Phase 1.5 — BATCH. Groups scanned files into LLM-friendly
// batches. Mirrors UA's compute-batches.mjs but trimmed to our
// needs:
//   - Group by file_category first (code / config / docs / infra /
//     data / script / markup) so each batch is a single category —
//     the LLM is given category-specific instructions.
//   - Within a category, sort by path (deterministic across runs).
//   - Pack into batches of at most `batchSize` files (UA default
//     is 10-15; we use 15 by default).
//   - When `--changed-files` is passed, the batch is a subset
//     containing only changed files; the resulting batches still
//     reference the full file inventory so cross-batch edges stay
//     resolvable (UA pattern).
//
// The output is a `BatchesPlan` JSON written to
// `.understand/batches.json`. Phase 2 reads it.

use std::collections::BTreeMap;
use std::fs;

use serde::{Deserialize, Serialize};

use crate::commands::code_wiki_scanner::ScannedFile;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BatchEntry {
    pub batch_index: u32,
    /// `"code" | "config" | "docs" | "infra" | "data" | "script" | "markup"`.
    pub category: String,
    /// File paths included in this batch.
    pub files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BatchesPlan {
    pub batch_size: u32,
    pub total_files: u32,
    pub total_batches: u32,
    pub by_category: BTreeMap<String, u32>,
    pub batches: Vec<BatchEntry>,
}

/// Compute the batch plan. `changed_only`, when non-empty, restricts
/// the input to those paths. Path comparison is normalised
/// (forward slashes) and case-insensitive on the prefix segment.
pub fn plan_batches_inner(
    files: &[ScannedFile],
    batch_size: u32,
    changed_only: &[String],
) -> BatchesPlan {
    let batch_size = batch_size.max(1);
    let filter: Option<std::collections::HashSet<String>> = if changed_only.is_empty() {
        None
    } else {
        Some(changed_only.iter().map(|s| s.replace('\\', "/")).collect())
    };
    let relevant: Vec<&ScannedFile> = files
        .iter()
        .filter(|f| match &filter {
            None => true,
            Some(set) => set.contains(&f.path),
        })
        .collect();
    // Group by category, preserving the file's original order
    // (which is already sorted by path inside the scanner).
    let mut by_category: BTreeMap<String, Vec<&ScannedFile>> = BTreeMap::new();
    for f in &relevant {
        by_category
            .entry(f.file_category.clone())
            .or_default()
            .push(f);
    }
    let mut batches: Vec<BatchEntry> = Vec::new();
    let mut next_idx: u32 = 0;
    let mut category_counts: BTreeMap<String, u32> = BTreeMap::new();
    for (category, mut files_in_cat) in by_category {
        // Sort within category by path (deterministic).
        files_in_cat.sort_by(|a, b| a.path.cmp(&b.path));
        for chunk in files_in_cat.chunks(batch_size as usize) {
            let files: Vec<String> = chunk.iter().map(|f| f.path.clone()).collect();
            batches.push(BatchEntry {
                batch_index: next_idx,
                category: category.clone(),
                files,
            });
            *category_counts.entry(category.clone()).or_insert(0) += 1;
            next_idx += 1;
        }
    }
    BatchesPlan {
        batch_size,
        total_files: relevant.len() as u32,
        total_batches: batches.len() as u32,
        by_category: category_counts,
        batches,
    }
}

pub fn write_batches_plan(path: &std::path::Path, plan: &BatchesPlan) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create dir: {e}"))?;
    }
    let json = serde_json::to_vec_pretty(plan).map_err(|e| format!("serialize: {e}"))?;
    fs::write(path, json).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, cat: &str) -> ScannedFile {
        ScannedFile {
            path: path.to_string(),
            language: "typescript".to_string(),
            size_lines: 10,
            file_category: cat.to_string(),
        }
    }

    #[test]
    fn batches_group_by_category() {
        let files = vec![
            f("src/a.ts", "code"),
            f("src/b.ts", "code"),
            f("README.md", "docs"),
            f("tsconfig.json", "config"),
        ];
        let plan = plan_batches_inner(&files, 10, &[]);
        assert_eq!(plan.total_files, 4);
        assert_eq!(plan.total_batches, 3); // 1 code, 1 docs, 1 config
        let code = plan.batches.iter().find(|b| b.category == "code").unwrap();
        assert_eq!(code.files.len(), 2);
    }

    #[test]
    fn batches_split_large_categories() {
        let files: Vec<_> = (0..30)
            .map(|i| f(&format!("src/file_{i:02}.ts"), "code"))
            .collect();
        let plan = plan_batches_inner(&files, 10, &[]);
        assert_eq!(plan.total_batches, 3);
        assert_eq!(plan.batches[0].files.len(), 10);
        assert_eq!(plan.batches[1].files.len(), 10);
        assert_eq!(plan.batches[2].files.len(), 10);
    }

    #[test]
    fn batches_renumber_across_categories() {
        let files = vec![
            f("a.ts", "code"),
            f("b.md", "docs"),
            f("c.ts", "code"),
        ];
        let plan = plan_batches_inner(&files, 1, &[]);
        // Sort: code first (a.ts, c.ts), then docs (b.md). 3 batches.
        let idx: Vec<u32> = plan.batches.iter().map(|b| b.batch_index).collect();
        assert_eq!(idx, vec![0, 1, 2]);
        assert_eq!(plan.batches[0].category, "code");
        assert_eq!(plan.batches[1].category, "code");
        assert_eq!(plan.batches[2].category, "docs");
    }

    #[test]
    fn batches_filter_changed_only() {
        let files = vec![
            f("src/a.ts", "code"),
            f("src/b.ts", "code"),
            f("src/c.ts", "code"),
        ];
        let plan = plan_batches_inner(&files, 10, &["src/b.ts".to_string()]);
        assert_eq!(plan.total_files, 1);
        assert_eq!(plan.batches[0].files, vec!["src/b.ts".to_string()]);
    }

    #[test]
    fn batches_normalize_path_separators() {
        let files = vec![f("src/a.ts", "code")];
        let plan = plan_batches_inner(&files, 10, &["src\\a.ts".to_string()]);
        assert_eq!(plan.total_files, 1);
    }

    #[test]
    fn batches_handle_empty_input() {
        let plan = plan_batches_inner(&[], 10, &[]);
        assert_eq!(plan.total_files, 0);
        assert_eq!(plan.total_batches, 0);
        assert!(plan.batches.is_empty());
    }

    #[test]
    fn batches_respect_explicit_batch_size_one() {
        let files = vec![f("a.ts", "code"), f("b.ts", "code")];
        let plan = plan_batches_inner(&files, 1, &[]);
        assert_eq!(plan.total_batches, 2);
    }
}
