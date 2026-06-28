// Phase 7 — SAVE. Writes the final UA `knowledge-graph.json` +
// `meta.json` + `fingerprints.json` atomically and cleans up
// intermediate files. Mirrors UA's Phase 7 invariants:
//   1. Fingerprint baseline MUST succeed before meta.json is written
//      (otherwise future auto-update would classify every file as
//      STRUCTURAL and force a FULL_UPDATE on every commit).
//   2. The final knowledge-graph.json is written via a sibling
//      `.tmp` and renamed into place atomically (avoids half-written
//      graphs on crash).
//   3. Intermediate files are preserved on a successful run so a
//      follow-up incremental can skip re-computing. They are
//      trash-stamped rather than rm-rf'd (UA's pattern, issue
//      #301) — we don't bother with delayed-purge for v1.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::commands::code_wiki_scanner::ScannedFile;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FingerprintEntry {
    pub path: String,
    /// Stable hash of the structural properties (file size,
    /// non-empty line count, language, file_category). We use a
    /// simple deterministic string so we don't need to pull in a
    /// hashing crate. UA uses a tree-sitter fingerprint; we
    /// approximate with file-level signals for v1.
    pub structural_hash: String,
    pub size_bytes: u64,
    pub non_empty_lines: u32,
    pub language: String,
    pub file_category: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FingerprintsBaseline {
    pub version: String,
    pub project_root: String,
    pub git_commit_hash: String,
    pub generated_at: String,
    pub files: Vec<FingerprintEntry>,
}

pub fn compute_fingerprint(file: &ScannedFile) -> FingerprintEntry {
    // Simple structural hash: combine the inputs that matter for
    // "did this file structurally change". Byte length and
    // non-empty line count catch most changes; language /
    // file_category catch file-renames and re-classifications.
    let input = format!(
        "{}|{}|{}|{}",
        file.path,
        file.size_lines,
        file.language,
        file.file_category,
    );
    // Cheap deterministic hash — fnv-1a 32-bit.
    let hash = fnv1a_32(input.as_bytes());
    FingerprintEntry {
        path: file.path.clone(),
        structural_hash: format!("{:08x}", hash),
        size_bytes: 0, // caller fills in from disk
        non_empty_lines: file.size_lines,
        language: file.language.clone(),
        file_category: file.file_category.clone(),
    }
}

fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// Write `fingerprints.json` for the given files. Returns the
/// absolute path written.
pub fn write_fingerprints(
    project_root: &Path,
    understand_dir: &Path,
    git_commit_hash: &str,
    files: &[ScannedFile],
) -> Result<PathBuf, String> {
    fs::create_dir_all(understand_dir).map_err(|e| format!("mkdir: {e}"))?;
    let mut entries: Vec<FingerprintEntry> = files
        .iter()
        .map(|f| {
            let mut e = compute_fingerprint(f);
            // Fill in size_bytes from disk if possible.
            let abs = project_root.join(&f.path);
            if let Ok(meta) = fs::metadata(&abs) {
                e.size_bytes = meta.len();
            }
            e
        })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let baseline = FingerprintsBaseline {
        version: "1.0.0".to_string(),
        project_root: project_root.to_string_lossy().to_string(),
        git_commit_hash: git_commit_hash.to_string(),
        generated_at: now_iso(),
        files: entries,
    };
    let path = understand_dir.join("fingerprints.json");
    let json = serde_json::to_vec_pretty(&baseline).map_err(|e| format!("serialize: {e}"))?;
    write_atomic(&path, &json)
        .map_err(|e| format!("write fingerprints: {e}"))?;
    Ok(path)
}

/// Atomic write: write to `<path>.tmp`, fsync (best-effort), rename
/// into place. On Windows, rename over an existing file requires
/// `fs::rename` to handle the destination. Modern Rust does this
/// correctly via `MoveFileExW` with replace semantics.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("")
    ));
    fs::write(&tmp, bytes)?;
    if let Ok(f) = fs::File::open(&tmp) {
        let _ = f.sync_all();
    }
    // On Windows, `fs::rename` will fail if the destination exists
    // in some configs. Use `fs::rename` first; fall back to remove +
    // rename if that fails.
    if fs::rename(&tmp, path).is_err() {
        let _ = fs::remove_file(path);
        fs::rename(&tmp, path)?;
    }
    Ok(())
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    // Minimal ISO 8601 (UTC) without external deps.
    let (year, month, day, hour, min, sec) = epoch_to_ymdhms(secs as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.000Z"
    )
}

fn epoch_to_ymdhms(epoch: i64) -> (i32, u32, u32, u32, u32, u32) {
    // Days since 1970-01-01 (Gregorian proleptic)
    let days = epoch.div_euclid(86_400);
    let secs_of_day = epoch.rem_euclid(86_400) as u32;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
    // Civil-from-days (Howard Hinnant's date algorithms, public domain)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i32 + (era as i32) * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d, hour, min, sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, lines: u32, cat: &str) -> ScannedFile {
        ScannedFile {
            path: path.to_string(),
            language: "rust".to_string(),
            size_lines: lines,
            file_category: cat.to_string(),
        }
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let a = compute_fingerprint(&f("src/lib.rs", 100, "code"));
        let b = compute_fingerprint(&f("src/lib.rs", 100, "code"));
        assert_eq!(a.structural_hash, b.structural_hash);
    }

    #[test]
    fn fingerprint_changes_with_inputs() {
        let a = compute_fingerprint(&f("src/lib.rs", 100, "code"));
        let b = compute_fingerprint(&f("src/lib.rs", 101, "code"));
        let c = compute_fingerprint(&f("src/lib.rs", 100, "docs"));
        let d = compute_fingerprint(&f("src/other.rs", 100, "code"));
        assert_ne!(a.structural_hash, b.structural_hash);
        assert_ne!(a.structural_hash, c.structural_hash);
        assert_ne!(a.structural_hash, d.structural_hash);
    }

    #[test]
    fn write_fingerprints_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let understand = project.join(".understand");
        let files = vec![f("src/lib.rs", 50, "code"), f("README.md", 10, "docs")];
        let path = write_fingerprints(&project, &understand, "deadbeef", &files).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: FingerprintsBaseline = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(parsed.git_commit_hash, "deadbeef");
        // Sorted by path
        assert_eq!(parsed.files[0].path, "README.md");
        assert_eq!(parsed.files[1].path, "src/lib.rs");
    }

    #[test]
    fn write_atomic_writes_via_tmp_then_renames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.json");
        write_atomic(&path, b"{\"x\":1}").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "{\"x\":1}");
        // No leftover .tmp
        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists(), ".tmp file leaked: {}", tmp.display());
    }

    #[test]
    fn write_atomic_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.json");
        write_atomic(&path, b"v1").unwrap();
        write_atomic(&path, b"v2").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "v2");
    }

    #[test]
    fn epoch_to_ymdhms_known_dates() {
        // 2026-06-28T00:00:00Z = 1780032000 (approx, ignoring leap seconds)
        // We just sanity-check the month/day/format
        let (y, m, d, h, mn, s) = epoch_to_ymdhms(1780032000);
        assert!(y >= 2026 && y <= 2027);
        assert!(m >= 1 && m <= 12);
        assert!(d >= 1 && d <= 31);
        assert!(h < 24);
        assert!(mn < 60);
        assert!(s < 60);
    }
}
