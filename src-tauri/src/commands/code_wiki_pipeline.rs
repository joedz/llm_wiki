// 7-phase pipeline orchestrator. Coordinates the scanner, ignore
// generator, batcher, and save phase; emits Tauri progress events
// for the UI; supports cooperative cancellation.
//
// M1 scope: Phases 0 / 0.5 / 1 / 1.5 / 7 run end-to-end. Phase 2
// (LLM file analysis) is a stub that runs the existing
// codegraph-only build path; the produced `knowledge-graph.json`
// has the new `project.frameworks` / `description` / `gitCommitHash`
// / `stats` fields populated. Phases 3-6 (assemble review, layers,
// tour, graph review) are M3 work and not invoked here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::task;

use crate::commands::code_wiki::{
    repo_root, source_dir_for, GRAPH_FILE, META_FILE,
};
use crate::commands::code_wiki_tree_sitter::build_graph_via_tree_sitter;
use crate::commands::code_wiki_analyzer::{analyze_batch as analyze_one_batch, FileEnrichment};
use crate::commands::code_wiki_architecture::{assign_layers, ArchitectureReport, Layer};
use crate::commands::code_wiki_assembler::assemble;
use crate::commands::code_wiki_batcher::{plan_batches_inner, write_batches_plan, BatchEntry};
use crate::commands::code_wiki_ignore::generate_understandignore_inner;
use crate::commands::code_wiki_reviewer::review_graph;
use crate::commands::code_wiki_save::{write_atomic, write_fingerprints, write_graph_streaming};
use crate::commands::code_wiki_scanner::{scan_project_inner, ScanResult};
use crate::commands::code_wiki_tour::{build_tour, TourStep};
use crate::llm_client::{LlmProvider, LlmRequest};

const UNDERSTAND_DIR: &str = ".understand";
const SCAN_RESULT_FILE: &str = "scan-result.json";
const BATCHES_FILE: &str = "batches.json";
const FINGERPRINTS_FILE: &str = "fingerprints.json";
const CONFIG_FILE: &str = "config.json";
const PIPELINE_EVENT: &str = "codewiki-pipeline-progress";

const DEFAULT_BATCH_SIZE: u32 = 15;
const DEFAULT_LLM_CONCURRENCY: u32 = 5;
const DEFAULT_LLM_BUDGET: u32 = 100;
/// P1-B: When the fingerprint-based diff finds this ratio (or
/// fewer) of files changed, Phase 2 LLM is skipped entirely and the
/// pipeline reuses the unchanged nodes from the prior
/// knowledge-graph.json. UA uses the same 10% heuristic.
const INCREMENTAL_LLM_SKIP_THRESHOLD: f32 = 0.1;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PipelineConfig {
    pub auto_update: bool,
    pub output_language: String,
    pub batch_size: u32,
    pub concurrency: u32,
    pub incremental: bool,
}

/// Tauri-friendly shape for the LLM request coming from TS.
/// Mirrors the chat panel's `LlmConfig` provider union; the
/// pipeline only needs the four fields required to make an
/// HTTP call. Other LlmConfig fields (apiMode, reasoning, etc.)
/// are ignored — the chat panel handles them.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LlmRequestSpec {
    pub provider: String, // "anthropic" | "openai" | "ollama" | "custom"
    #[serde(alias = "apiKey")]
    pub api_key: String,
    pub model: String,
    #[serde(default, alias = "baseUrl")]
    pub base_url: Option<String>,
    #[serde(default, alias = "maxTokens")]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
}

impl LlmRequestSpec {
    pub fn into_request(&self, system: String, user: String) -> LlmRequest {
        LlmRequest {
            provider: match self.provider.as_str() {
                "anthropic" => LlmProvider::Anthropic,
                "ollama" => LlmProvider::Ollama,
                "custom" => LlmProvider::Custom,
                _ => LlmProvider::Openai,
            },
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            system,
            user,
            max_tokens: self.max_tokens.unwrap_or(4096),
            temperature: self.temperature.unwrap_or(0.2),
        }
    }
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            auto_update: false,
            output_language: "en".to_string(),
            batch_size: DEFAULT_BATCH_SIZE,
            concurrency: 5,
            incremental: true,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PipelineSummary {
    pub pipeline_id: String,
    pub project_path: String,
    pub repo_name: String,
    pub final_graph_path: String,
    pub final_meta_path: String,
    pub final_fingerprints_path: String,
    pub node_count: u32,
    pub edge_count: u32,
    pub layer_count: u32,
    pub tour_step_count: u32,
    pub duration_ms: u64,
    pub cancelled: bool,
    pub warnings: Vec<String>,
    /// Optional LLM `--review` verdict (approved, issues,
    /// warnings, narrative). Populated only when the pipeline
    /// ran Phase 8.5 with `review_llm` set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_narrative: Option<serde_json::Value>,
    /// Optional Phase 5.5 LLM assemble-reviewer report
    /// (`types_remapped`, `complexity_remapped`,
    /// `cross_batch_edges_added`, `notes`). Populated when the
    /// pipeline ran with `assemble_review_llm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assemble_review: Option<serde_json::Value>,
}

#[derive(Default)]
pub struct PipelineRegistry {
    next_id: AtomicU32,
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl PipelineRegistry {
    pub fn new_id(&self) -> String {
        let n = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("pipeline-{n}")
    }
    pub fn register_cancel(&self, id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.cancels
            .lock()
            .expect("pipeline registry poisoned")
            .insert(id.to_string(), flag.clone());
        flag
    }
    pub fn cancel(&self, id: &str) -> bool {
        if let Some(flag) = self.cancels.lock().expect("poisoned").get(id) {
            flag.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
    pub fn unregister(&self, id: &str) {
        self.cancels.lock().expect("poisoned").remove(id);
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProgressEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        pipeline_id: String,
        repo_name: String,
        total_phases: u32,
    },
    #[serde(rename_all = "camelCase")]
    Phase {
        pipeline_id: String,
        phase: u32,
        label: String,
        status: String,
    },
    #[serde(rename_all = "camelCase")]
    Batch {
        pipeline_id: String,
        phase: u32,
        batch_index: u32,
        total_batches: u32,
        file_count: u32,
        status: String,
    },
    #[serde(rename_all = "camelCase")]
    Warning {
        pipeline_id: String,
        phase: u32,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    Cancelled {
        pipeline_id: String,
        phase: u32,
        partial_saved: bool,
    },
    #[serde(rename_all = "camelCase")]
    Done {
        pipeline_id: String,
        summary: PipelineSummary,
    },
}

fn emit(app: &AppHandle, event: &ProgressEvent) {
    let _ = app.emit(PIPELINE_EVENT, event);
}

pub fn understand_dir_for(repo_dir: &std::path::Path) -> PathBuf {
    repo_dir.join(UNDERSTAND_DIR)
}

fn read_config(understand_dir: &std::path::Path) -> PipelineConfig {
    let path = understand_dir.join(CONFIG_FILE);
    let Ok(raw) = std::fs::read_to_string(&path) else { return PipelineConfig::default() };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn write_config(understand_dir: &std::path::Path, cfg: &PipelineConfig) -> std::io::Result<()> {
    std::fs::create_dir_all(understand_dir)?;
    let bytes = serde_json::to_vec_pretty(cfg).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    write_atomic(&understand_dir.join(CONFIG_FILE), &bytes)
}

fn check_cancel(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::SeqCst)
}

// --- M1: UA-shape graph construction in Rust ----------------------------
//
// The TS `buildKnowledgeGraph` does the same mapping (see
// `src/lib/code-wiki/knowledge-graph-writer.ts`). For M1 the
// pipeline is self-contained — no IPC roundtrip through TS — so we
// reproduce the mapping here. M2 will refactor: the TS writer
// stays as the source of truth and the pipeline calls it via a
// thin Tauri command.

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct KnowledgeGraph {
    pub version: String,
    #[serde(rename = "kind")]
    pub kind: String,
    pub project: ProjectMeta,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub layers: Vec<Layer>,
    pub tour: Vec<TourStep>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProjectMeta {
    pub name: String,
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    pub description: String,
    #[serde(rename = "analyzedAt")]
    pub analyzed_at: String,
    #[serde(rename = "gitCommitHash")]
    pub git_commit_hash: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GraphNode {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub complexity: String,
    pub location: Option<NodeLocation>,
    pub language_notes: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodeLocation {
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub direction: String,
    pub weight: f32,
    /// Optional human-readable description (used by cross_domain
    /// edges in domain graphs and any LLM-emitted edge). Omitted
    /// from JSON when absent to keep the codebase graph slim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn codegraph_to_ua_kind(codegraph_kind: &str) -> Option<&'static str> {
    Some(match codegraph_kind {
        "file" => "file",
        "function" | "method" => "function",
        "class" | "struct" | "interface" | "type_alias" | "enum" | "enum_member" => "class",
        "module" => "module",
        "constant" | "variable" | "property" => "concept",
        "import" => "module",
        "component" => "service",
        "route" => "endpoint",
        _ => return None,
    })
}

fn codegraph_edge_to_ua(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "contains" => "contains",
        "imports" => "imports",
        "calls" => "calls",
        "inherits" => "inherits",
        "implements" => "implements",
        "exports" => "exports",
        _ => return None,
    })
}

// --- Pipeline execution -------------------------------------------------

/// Tauri command: run the 7-phase pipeline in a background tokio
/// task. The pipeline emits `codewiki-pipeline-progress` events on
/// `app` for every state change. Returns once the task is spawned;
/// the caller listens for the `Done` event to get the final summary.
///
/// When `llm` is provided, Phase 2 (analyze) is LLM-enriched:
/// each batch of files is sent to the LLM and the response is
/// applied as `summary`/`tags`/`complexity` on the file-level nodes.
/// When `llm` is `None`, Phase 2 falls back to the codegraph-only
/// build (M1 behavior).
///
/// `review_llm` (separate from `llm`): when set, Phase 8.5 fires
/// after the deterministic review (Phase 8) — the LLM gets the
/// deterministic findings plus a small graph summary and
/// produces an `approved` decision + narrative. Off by default.
#[tauri::command]
pub async fn code_wiki_run_pipeline(
    project_path: String,
    repo_name: String,
    llm: Option<LlmRequestSpec>,
    review_llm: Option<LlmRequestSpec>,
    assemble_review_llm: Option<LlmRequestSpec>,
    // P1-B: When true (default), the pipeline compares current scan
    // fingerprints against the prior baseline. If fewer than
    // INCREMENTAL_LLM_SKIP_THRESHOLD files changed AND an LLM was
    // requested, Phase 2 LLM is skipped and the prior graph's
    // unchanged nodes are spliced in. Pass false to force a full
    // rebuild (fresh repos, major restructure).
    incremental: Option<bool>,
    app: AppHandle,
    state: tauri::State<'_, Arc<PipelineRegistry>>,
) -> Result<(), String> {
    let registry = state.inner().clone();
    let app_for_task = app.clone();
    tokio::spawn(async move {
        let result = run_pipeline(
            app_for_task,
            registry,
            project_path,
            repo_name,
            llm,
            review_llm,
            assemble_review_llm,
            incremental.unwrap_or(true),
        )
        .await;
        if let Err(e) = result {
            eprintln!("[code-wiki pipeline] run_pipeline failed: {e}");
        }
    });
    Ok(())
}

/// Tauri command: cancel a running pipeline. Returns true if a
/// pipeline with that id was found and the cancel signal was set.
#[tauri::command]
pub async fn code_wiki_cancel_pipeline(
    pipeline_id: String,
    state: tauri::State<'_, Arc<PipelineRegistry>>,
) -> Result<bool, String> {
    Ok(state.cancel(&pipeline_id))
}

pub async fn run_pipeline(
    app: AppHandle,
    registry: Arc<PipelineRegistry>,
    project_path: String,
    repo_name: String,
    llm: Option<LlmRequestSpec>,
    review_llm: Option<LlmRequestSpec>,
    assemble_review_llm: Option<LlmRequestSpec>,
    incremental: bool,
) -> Result<PipelineSummary, String> {
    // Use project_path as the stable pipelineId — it must match what the
    // frontend store sets in begin() so all events (phase/batch/done) are
    // correctly routed to the right UI entry.
    let pipeline_id = project_path.clone();
    let cancel = registry.register_cancel(&pipeline_id);

    let app_for_task = app.clone();
    let pid_for_task = pipeline_id.clone();
    let project_for_task = project_path.clone();
    let repo_for_task = repo_name.clone();
    let registry_for_task = registry.clone();

    let result = run_pipeline_orchestrator(
        &app_for_task,
        &pid_for_task,
        &project_for_task,
        &repo_for_task,
        llm,
        review_llm,
        assemble_review_llm,
        incremental,
        &cancel,
        Instant::now(),
    )
    .await;
    registry_for_task.unregister(&pipeline_id);
    result
}

/// Async orchestrator. The phases up to and including 1.5 are
/// synchronous (file I/O only); Phase 2 (LLM) is async; Phase 7
/// is sync. We thread an `Instant` for the wall-clock duration.
async fn run_pipeline_orchestrator(
    app: &AppHandle,
    pipeline_id: &str,
    project_path: &str,
    repo_name: &str,
    llm: Option<LlmRequestSpec>,
    review_llm: Option<LlmRequestSpec>,
    assemble_review_llm: Option<LlmRequestSpec>,
    incremental: bool,
    cancel: &AtomicBool,
    started: Instant,
) -> Result<PipelineSummary, String> {
    let started = started;
    eprintln!("[pipeline:orch] run_pipeline_orchestrator START id={} project={} repo={}", pipeline_id, project_path, repo_name);
    emit(
        app,
        &ProgressEvent::Started {
            pipeline_id: pipeline_id.to_string(),
            repo_name: repo_name.to_string(),
            total_phases: 10,
        },
    );

    let mut warnings: Vec<String> = Vec::new();
    let project_root = PathBuf::from(project_path);
    if !project_root.is_dir() {
        return Err(format!("project path is not a directory: {project_path}"));
    }
    // repo_dir = output directory for knowledge-graph.json / .understand/
    let repo_dir = repo_root(&project_root, repo_name);
    if !repo_dir.is_dir() {
        return Err(format!("repo output dir not found: {}", repo_dir.display()));
    }
    // source_dir = actual source code to be analyzed
    let source_dir = source_dir_for(&project_root, repo_name);
    if !source_dir.is_dir() {
        return Err(format!("source code dir not found: {} (checked raw/code/{})", source_dir.display(), repo_name));
    }
    let understand_dir = understand_dir_for(&repo_dir);
    std::fs::create_dir_all(&understand_dir).map_err(|e| format!("mkdir .understand: {e}"))?;

    // --- Phase 0: pre-flight (config) ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    emit_phase(app, pipeline_id, 0, "Pre-flight", "running");
    let _ = write_config(&understand_dir, &PipelineConfig::default());
    emit_phase(app, pipeline_id, 0, "Pre-flight", "done");

    // --- Phase 0.5: ignore (project root) ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    emit_phase(app, pipeline_id, 1, "Ignore config", "running");
    if let Err(e) = generate_understandignore_inner(&project_root) {
        warnings.push(format!("ignore generation failed: {e}"));
        emit_warning(app, pipeline_id, 1, &warnings.last().unwrap());
    }
    emit_phase(app, pipeline_id, 1, "Ignore config", "done");

    // --- Phase 1: scan (the repo, since codegraph lives there) ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    emit_phase(app, pipeline_id, 2, "Scan", "running");
    let scan = match scan_project_inner(&source_dir) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("scan failed: {e}");
            emit_warning(app, pipeline_id, 2, &msg);
            return Err(msg);
        }
    };
    if let Err(e) = write_atomic(
        &understand_dir.join(SCAN_RESULT_FILE),
        &serde_json::to_vec_pretty(&scan).map_err(|e| format!("serialize scan: {e}"))?,
    ) {
        warnings.push(format!("failed to write scan-result.json: {e}"));
        emit_warning(app, pipeline_id, 2, &warnings.last().unwrap());
    }
    emit_phase(app, pipeline_id, 2, "Scan", "done");

    // --- Phase 1.5: batch ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    emit_phase(app, pipeline_id, 3, "Batch", "running");
    let plan = plan_batches_inner(&scan.files, DEFAULT_BATCH_SIZE, &[]);
    if let Err(e) = write_batches_plan(&understand_dir.join(BATCHES_FILE), &plan) {
        warnings.push(format!("failed to write batches.json: {e}"));
        emit_warning(app, pipeline_id, 3, &warnings.last().unwrap());
    }
    for batch in &plan.batches {
        emit_batch(
            app,
            pipeline_id,
            3,
            batch.batch_index,
            plan.total_batches,
            batch.files.len() as u32,
            "done",
        );
    }
    emit_phase(app, pipeline_id, 3, "Batch", "done");

    // --- P1-B: incremental diff (moved up from Phase 9 so we can
    // decide whether to skip Phase 2 LLM before paying its cost) ---
    //
    // We load the prior fingerprints.json (if any) and compare it
    // against the current scan. The result is a snapshot of
    // changed / unchanged / removed paths plus the decision of
    // whether Phase 2 LLM should be skipped (low change ratio +
    // LLM requested + fingerprints baseline exists + incremental
    // requested).
    let baseline = std::fs::read_to_string(&understand_dir.join(FINGERPRINTS_FILE))
        .ok()
        .and_then(|raw| {
            serde_json::from_str::<crate::commands::code_wiki_save::FingerprintsBaseline>(&raw).ok()
        });
    let (changed_paths, unchanged_paths, removed_paths) =
        crate::commands::code_wiki_scanner::compute_changed_files(&scan, baseline.as_ref());
    let total_code_files = scan
        .files
        .iter()
        .filter(|f| f.file_category == "code")
        .count() as u32;

    let mut phase2_skipped = false;
    let mut phase2_skip_reason: Option<String> = None;
    let skip_phase2_llm = incremental
        && llm.is_some()
        && baseline.is_some()
        && total_code_files > 0
        && !changed_paths.is_empty()
        && (changed_paths.len() as f32) / (total_code_files as f32)
            < INCREMENTAL_LLM_SKIP_THRESHOLD;

    // --- Phase 2 ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    let phase2_label = if skip_phase2_llm {
        "Analyze (no LLM — incremental)"
    } else if llm.is_some() {
        "Analyze (LLM)"
    } else {
        "Analyze (no LLM)"
    };
    emit_phase(app, pipeline_id, 4, phase2_label, "running");

    let mut graph = match build_graph_via_tree_sitter(
        &project_root,
        repo_name,
        &scan,
    ) {
        Ok(g) => g,
        Err(e) => {
            let msg = format!("phase 2 tree-sitter build failed: {e}");
            emit_warning(app, pipeline_id, 4, &msg);
            return Err(msg);
        }
    };

    if skip_phase2_llm {
        // Reuse unchanged file-level nodes from the prior graph.
        // The deterministic tree-sitter rebuild produced nodes
        // for *all* files in `scan` (we don't have an incremental
        // build path yet), so we remove the unchanged ones and
        // splice in the prior versions instead. This preserves
        // LLM-written summary / tags / complexity for unchanged
        // files without re-running Phase 2.
        let reason = format!(
            "{} changed / {} total code files = {:.1}% < {:.0}% threshold; reusing prior graph nodes",
            changed_paths.len(),
            total_code_files,
            (changed_paths.len() as f32) / (total_code_files.max(1) as f32) * 100.0,
            INCREMENTAL_LLM_SKIP_THRESHOLD * 100.0
        );
        phase2_skipped = true;
        phase2_skip_reason = Some(reason.clone());
        warnings.push(format!("Phase 2 LLM skipped: {reason}"));
        emit_warning(app, pipeline_id, 4, &reason);

        match splice_unchanged_nodes(
            &repo_dir,
            &mut graph,
            &changed_paths,
            &removed_paths,
        ) {
            Ok(spliced) => {
                if spliced > 0 {
                    let msg = format!(
                        "Spliced {} unchanged file nodes from prior knowledge-graph.json",
                        spliced
                    );
                    warnings.push(msg.clone());
                    emit_warning(app, pipeline_id, 4, &msg);
                }
            }
            Err(e) => {
                let msg = format!("prior graph splice failed: {e}");
                warnings.push(msg.clone());
                emit_warning(app, pipeline_id, 4, &msg);
            }
        }
    } else if let Some(llm_spec) = llm {
        if plan.batches.len() as u32 > DEFAULT_LLM_BUDGET {
            let msg = format!(
                "Batch count {} exceeds LLM budget cap {}; truncating to first {}",
                plan.batches.len(),
                DEFAULT_LLM_BUDGET,
                DEFAULT_LLM_BUDGET
            );
            emit_warning(app, pipeline_id, 4, &msg);
            warnings.push(msg);
        }
        let runnable_batches: Vec<&BatchEntry> = plan
            .batches
            .iter()
            .take(DEFAULT_LLM_BUDGET as usize)
            .collect();
        let runnable_total = runnable_batches.len() as u32;
        match run_phase2_llm(
            app,
            pipeline_id,
            &source_dir,
            &scan,
            &runnable_batches,
            runnable_total,
            &llm_spec,
            cancel,
            &mut warnings,
        )
        .await
        {
            Ok(enrichments) => {
                apply_enrichments(&mut graph, &enrichments);
            }
            Err(msg) => {
                emit_warning(app, pipeline_id, 4, &msg);
                warnings.push(msg);
            }
        }
    }
    emit_phase(app, pipeline_id, 4, phase2_label, "done");

    // --- Phase 5: assemble (dedup, validate, normalize) ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    emit_phase(app, pipeline_id, 5, "Assemble", "running");
    let (graph, asm_report) = assemble(graph);
    if asm_report.nodes_deduped > 0
        || asm_report.edges_deduped > 0
        || asm_report.edges_dropped > 0
        || asm_report.nodes_renamed > 0
        || asm_report.complexity_normalized > 0
    {
        let msg = format!(
            "Assemble: {} nodes renamed, {} nodes deduped, {} edges deduped, {} edges dropped, {} complexities normalized",
            asm_report.nodes_renamed,
            asm_report.nodes_deduped,
            asm_report.edges_deduped,
            asm_report.edges_dropped,
            asm_report.complexity_normalized,
        );
        warnings.push(msg.clone());
        emit_warning(app, pipeline_id, 5, &msg);
    }
    emit_phase(app, pipeline_id, 5, "Assemble", "done");

    // --- Phase 5.5: assemble-reviewer (LLM post-merge cleanup) ---
    let mut graph = graph;
    let mut assemble_review_value: Option<serde_json::Value> = None;
    if let Some(ref spec) = assemble_review_llm {
        if check_cancel(cancel) {
            return Ok(cancelled_summary(
                pipeline_id,
                project_path,
                repo_name,
                started,
                &warnings,
            ));
        }
        emit_phase(app, pipeline_id, 5, "Assemble review (LLM)", "running");
        let review = crate::commands::code_wiki_assemble_llm::assemble_review_llm(
            &mut graph,
            &asm_report,
            &scan,
            spec,
        )
        .await;
        assemble_review_value = Some(serde_json::to_value(&review).unwrap_or_default());
        let summary = format!(
            "Assemble review: {} types remapped, {} complexities remapped, {} cross-batch imports added",
            review.types_remapped, review.complexity_remapped, review.cross_batch_edges_added
        );
        warnings.push(summary.clone());
        emit_warning(app, pipeline_id, 5, &summary);
        for note in &review.notes {
            let m = format!("Assemble review note: {note}");
            warnings.push(m.clone());
            emit_warning(app, pipeline_id, 5, &m);
        }
        emit_phase(app, pipeline_id, 5, "Assemble review (LLM)", "done");
    }

    // --- Phase 6: architecture (assign layers) ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    emit_phase(app, pipeline_id, 6, "Architecture", "running");
    let arch_report: ArchitectureReport = assign_layers(&graph);
    graph.layers = arch_report.layers.clone();
    if arch_report.unassigned > 0 {
        let msg = format!(
            "{} file-level nodes could not be assigned to a layer (missing file_path)",
            arch_report.unassigned
        );
        warnings.push(msg.clone());
        emit_warning(app, pipeline_id, 6, &msg);
    }
    emit_phase(app, pipeline_id, 6, "Architecture", "done");

    // --- Phase 7: tour (build guided walkthrough) ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    emit_phase(app, pipeline_id, 7, "Tour", "running");
    let tour_report = build_tour(&graph);
    graph.tour = tour_report.steps.clone();
    if tour_report.truncated {
        warnings.push("Tour was truncated to MAX_STEPS".to_string());
    }
    emit_phase(app, pipeline_id, 7, "Tour", "done");

    // --- Phase 8: review (deterministic validation) ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    emit_phase(app, pipeline_id, 8, "Review", "running");
    let review = review_graph(&graph, &graph.layers, &graph.tour);
    let error_count = review.issues.iter().filter(|i| i.level == "error").count();
    let warning_count = review.issues.iter().filter(|i| i.level == "warning").count();
    if error_count > 0 || warning_count > 0 {
        let msg = format!(
            "Review: {} errors, {} warnings",
            error_count, warning_count
        );
        warnings.push(msg.clone());
        emit_warning(app, pipeline_id, 8, &msg);
    }
    // Embed stats as a `stats` top-level field on the final graph
    // (UA's `knowledge-graph.json` carries stats inline).
    let stats_value = serde_json::to_value(&review.stats)
        .map_err(|e| format!("serialize stats: {e}"))?;
    emit_phase(app, pipeline_id, 8, "Review", "done");
    let _ = stats_value;

    // --- Phase 8.5: LLM review (UA `--review` mode) ---
    //
    // Optional — only runs when `review_llm` is provided. LLM
    // reviews the deterministic findings + a small graph
    // summary and produces `{approved, issues, warnings, narrative}`.
    // LLM errors are recorded as warnings; the pipeline never
    // fails on LLM error.
    let mut review_narrative: Option<serde_json::Value> = None;
    if let Some(spec) = review_llm {
        if check_cancel(cancel) {
            return Ok(cancelled_summary(
                pipeline_id, project_path, repo_name, started, &warnings,
            ));
        }
        emit_phase(app, pipeline_id, 8, "LLM Review", "running");
        match crate::commands::code_wiki_reviewer_llm::call_graph_reviewer(&spec, &review, &graph).await {
            Ok(verdict) => {
                if !verdict.approved {
                    warnings.push(format!(
                        "LLM review: not approved ({} issues, {} warnings)",
                        verdict.issues.len(),
                        verdict.warnings.len()
                    ));
                }
                for w in &verdict.warnings {
                    warnings.push(format!("LLM review: {w}"));
                }
                review_narrative =
                    Some(crate::commands::code_wiki_reviewer_llm::narrative_for_meta(&verdict));
            }
            Err(e) => {
                warnings.push(format!("LLM review failed: {e}"));
            }
        }
        emit_phase(app, pipeline_id, 8, "LLM Review", "done");
    }

    // --- Phase 9 ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    emit_phase(app, pipeline_id, 9, "Save", "running");
    // P1-B: the diff was already computed at the top of Phase 2;
    // here we just persist the counts to meta.json so the dashboard
    // can show "X files changed since last build".
    let changed_count = changed_paths.len() as u32;
    let unchanged_count = unchanged_paths.len() as u32;
    let removed_count = removed_paths.len() as u32;
    if !changed_paths.is_empty() || !removed_paths.is_empty() {
        let msg = format!(
            "Incremental diff: {} changed, {} unchanged, {} removed since last build",
            changed_count,
            unchanged_count,
            removed_count
        );
        warnings.push(msg.clone());
        emit_warning(app, pipeline_id, 9, &msg);
    }
    let fp_path = match write_fingerprints(
        &project_root,
        &understand_dir,
        &scan.git_commit_hash,
        &scan.files,
    ) {
        Ok(p) => p,
        Err(e) => {
            warnings.push(format!("fingerprints failed: {e}"));
            emit_warning(app, pipeline_id, 9, &warnings.last().unwrap());
            understand_dir.join(FINGERPRINTS_FILE)
        }
    };
    let graph_path = repo_dir.join(GRAPH_FILE);
    if let Err(e) = write_graph_streaming(&graph_path, &graph) {
        let msg = format!("graph write failed: {e}");
        emit_warning(app, pipeline_id, 9, &msg);
        return Err(msg);
    }
    let meta = crate::commands::code_wiki_save::PipelineMeta {
        last_analyzed_at: now_iso(),
        git_commit_hash: scan.git_commit_hash.clone(),
        version: "codewiki-1.0.0".to_string(),
        kind: "codebase".to_string(),
        analyzed_files: scan.files.iter().filter(|f| f.file_category == "code").count() as u32,
        review_narrative: review_narrative.clone(),
        review_approved: review_narrative
            .as_ref()
            .and_then(|v| v.get("approved").and_then(|x| x.as_bool())),
        assemble_review: assemble_review_value.clone(),
        changed_file_count: Some(changed_count),
        unchanged_file_count: Some(unchanged_count),
        removed_file_count: Some(removed_count),
        phase2_skipped_due_to_incremental: if phase2_skipped { Some(true) } else { None },
        phase2_skip_reason,
    };
    let meta_path = repo_dir.join(META_FILE);
    if let Err(e) = crate::commands::code_wiki_save::write_meta(&meta_path, &meta) {
        warnings.push(format!("meta.json write failed: {e}"));
        emit_warning(app, pipeline_id, 9, &warnings.last().unwrap());
    }
    emit_phase(app, pipeline_id, 9, "Save", "done");

    let summary = PipelineSummary {
        pipeline_id: pipeline_id.to_string(),
        project_path: project_path.to_string(),
        repo_name: repo_name.to_string(),
        final_graph_path: graph_path.to_string_lossy().to_string(),
        final_meta_path: meta_path.to_string_lossy().to_string(),
        final_fingerprints_path: fp_path.to_string_lossy().to_string(),
        node_count: graph.nodes.len() as u32,
        edge_count: graph.edges.len() as u32,
        layer_count: graph.layers.len() as u32,
        tour_step_count: graph.tour.len() as u32,
        duration_ms: started.elapsed().as_millis() as u64,
        cancelled: false,
        warnings: warnings.clone(),
        review_narrative: review_narrative.clone(),
        assemble_review: assemble_review_value.clone(),
    };

    // Auto-refresh the diff overlay so the dashboard's diff view
    // is up-to-date. Best-effort: failures don't block the
    // "Done" event.
    if let Err(e) = crate::commands::code_wiki_diff::refresh_diff_overlay_inner(
        &project_root,
        repo_name,
        None,
    ) {
        eprintln!("[code-wiki pipeline] diff overlay refresh failed: {e}");
    }

    emit(
        app,
        &ProgressEvent::Done {
            pipeline_id: pipeline_id.to_string(),
            summary: summary.clone(),
        },
    );
    Ok(summary)
}

/// Apply LLM-produced enrichments to the in-memory graph.
/// Enrichments carry summary / tags / complexity for the file
/// node matched by `path`, plus optional `reads_from` / `writes_to`
/// arrays (P1-A) — each entry becomes a cross-file data-flow edge
/// from this file's node to the target file's node. Edges whose
/// target is missing from the graph are silently dropped here; the
/// assembler (Phase 5) will dedupe / drop dangling for the
/// deterministic edge sources.
fn apply_enrichments(graph: &mut KnowledgeGraph, enrichments: &[FileEnrichment]) {
    for enr in enrichments {
        if let Some(node) = graph.nodes.iter_mut().find(|n| n.file_path == enr.path) {
            node.summary = enr.summary.clone();
            node.tags = enr.tags.clone();
            node.complexity = enr.complexity.clone();
        }
    }

    // P1-A: emit reads_from / writes_to edges. We need the source
    // file node id and the valid_node_ids set so we can avoid
    // dangling targets. We collect new edges into a Vec so the
    // borrow checker is happy (graph.nodes is borrowed mutably for
    // the enrichments above, but here we only borrow immutably).
    let valid_node_ids: std::collections::HashSet<String> =
        graph.nodes.iter().map(|n| n.id.clone()).collect();
    let existing_edges: std::collections::HashSet<(String, String, String)> = graph
        .edges
        .iter()
        .map(|e| (e.source.clone(), e.target.clone(), e.kind.clone()))
        .collect();
    let mut new_edges: Vec<GraphEdge> = Vec::new();
    for enr in enrichments {
        let source_id = format!("file:{}", enr.path);
        if !valid_node_ids.contains(&source_id) {
            continue;
        }
        for tgt_rel in enr.reads_from.iter().chain(enr.writes_to.iter()) {
            let target_id = format!("file:{tgt_rel}");
            if !valid_node_ids.contains(&target_id) {
                continue;
            }
            if source_id == target_id {
                continue;
            }
            let kind = if enr.reads_from.iter().any(|r| r == tgt_rel) {
                "reads_from"
            } else {
                "writes_to"
            };
            let key = (source_id.clone(), target_id.clone(), kind.to_string());
            if existing_edges.contains(&key) {
                continue;
            }
            new_edges.push(GraphEdge {
                source: source_id.clone(),
                target: target_id,
                kind: kind.to_string(),
                direction: "forward".to_string(),
                weight: 0.5,
                description: None,
            });
        }
    }
    graph.edges.extend(new_edges);
}

/// P1-B: Splice file-level nodes for unchanged paths back into the
/// graph from the prior `knowledge-graph.json`. We replace the
/// freshly-built nodes for unchanged files (which lack LLM-written
/// summary / tags / complexity) with their prior versions. The
/// assembler will dedupe / drop dangling as usual.
///
/// Returns the count of nodes actually replaced.
fn splice_unchanged_nodes(
    repo_dir: &std::path::Path,
    graph: &mut KnowledgeGraph,
    changed_paths: &[String],
    _removed_paths: &[String],
) -> Result<u32, String> {
    let graph_path = repo_dir.join(GRAPH_FILE);
    let raw = match std::fs::read_to_string(&graph_path) {
        Ok(s) => s,
        Err(_) => {
            // No prior graph → nothing to splice (caller's warning
            // already covers this case in practice; we still want
            // the function to be infallible in spirit).
            return Ok(0);
        }
    };
    let prior: KnowledgeGraph = match serde_json::from_str(&raw) {
        Ok(g) => g,
        Err(e) => return Err(format!("parse prior graph: {e}")),
    };

    let changed_set: std::collections::HashSet<&str> =
        changed_paths.iter().map(|s| s.as_str()).collect();

    let mut spliced: u32 = 0;
    for prior_node in prior
        .nodes
        .iter()
        .filter(|n| n.kind == "file" && !changed_set.contains(n.file_path.as_str()))
    {
        // Replace the matching freshly-built node (if any) with the
        // prior version. If no fresh node exists for this path,
        // append the prior version directly.
        let target_id = format!("file:{}", prior_node.file_path);
        if let Some(fresh) = graph
            .nodes
            .iter_mut()
            .find(|n| n.id == target_id && n.kind == "file")
        {
            *fresh = prior_node.clone();
        } else {
            graph.nodes.push(prior_node.clone());
        }
        spliced += 1;
    }

    Ok(spliced)
}

/// Run Phase 2 LLM enrichment for a list of batches. Up to
/// `DEFAULT_LLM_CONCURRENCY` (5) batches run in parallel. Each
/// batch emits a `Batch` progress event. Per-batch failures are
/// recorded as warnings; the caller decides whether to abort.
async fn run_phase2_llm(
    app: &AppHandle,
    pipeline_id: &str,
    project_root: &Path,
    scan: &ScanResult,
    batches: &[&BatchEntry],
    total: u32,
    llm_spec: &LlmRequestSpec,
    cancel: &AtomicBool,
    warnings: &mut Vec<String>,
) -> Result<Vec<FileEnrichment>, String> {
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    // Build a path -> ScannedFile lookup. The ScannedFile is
    // borrowed by the analysis tasks; we clone the file paths
    // into owned ScannedFile values per task to avoid lifetime
    // issues with concurrent borrows.
    let paths: Vec<String> = batches
        .iter()
        .flat_map(|b| b.files.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let _ = paths; // not needed directly; the batch loop reads them

    let sem = Arc::new(Semaphore::new(DEFAULT_LLM_CONCURRENCY as usize));
    let mut tasks = tokio::task::JoinSet::new();

    for batch in batches {
        if check_cancel(cancel) {
            break;
        }
        let permit = sem.clone().acquire_owned().await.map_err(|e| {
            format!("acquire semaphore permit: {e}")
        })?;
        let batch = (*batch).clone();
        let project = project_root.to_path_buf();
        let llm_spec = llm_spec.clone();
        let scan_files: Vec<_> = scan
            .files
            .iter()
            .filter(|f| batch.files.contains(&f.path))
            .cloned()
            .collect();
        let files_by_path: HashMap<String, crate::commands::code_wiki_scanner::ScannedFile> =
            scan_files.iter().map(|f| (f.path.clone(), f.clone())).collect();
        let app = app.clone();
        let pid = pipeline_id.to_string();
        let total_u32 = total;

        tasks.spawn(async move {
            let _permit = permit; // dropped on task completion
            emit_batch(&app, &pid, 4, batch.batch_index, total_u32, batch.files.len() as u32, "running");
            let system = "You are an expert code analyst.".to_string();
            let req = llm_spec.into_request(system, String::new());
            // Borrow the owned ScannedFile map as &ScannedFile values
            // for the analyzer. This is safe because `files_by_path`
            // is moved into the task and outlives the analyzer call.
            let refs: HashMap<String, &crate::commands::code_wiki_scanner::ScannedFile> =
                files_by_path.iter().map(|(k, v)| (k.clone(), v)).collect();
            let result = analyze_one_batch(&batch, &project, &refs, req).await;
            let status = if result.is_ok() { "done" } else { "error" };
            emit_batch(
                &app,
                &pid,
                4,
                batch.batch_index,
                total_u32,
                batch.files.len() as u32,
                status,
            );
            (batch.batch_index, result)
        });
    }

    let mut enrichments: Vec<FileEnrichment> = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let (_idx, result) = match joined {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("LLM batch task panicked: {e}"));
                continue;
            }
        };
        match result {
            Ok(enr) => enrichments.extend(enr.enrichments),
            Err(e) => warnings.push(format!("LLM batch failed: {e}")),
        }
    }
    Ok(enrichments)
}

fn cancelled_summary(
    pipeline_id: &str,
    project_path: &str,
    repo_name: &str,
    started: Instant,
    warnings: &[String],
) -> PipelineSummary {
    PipelineSummary {
        pipeline_id: pipeline_id.to_string(),
        project_path: project_path.to_string(),
        repo_name: repo_name.to_string(),
        final_graph_path: String::new(),
        final_meta_path: String::new(),
        final_fingerprints_path: String::new(),
        node_count: 0,
        edge_count: 0,
        layer_count: 0,
        tour_step_count: 0,
        duration_ms: started.elapsed().as_millis() as u64,
        cancelled: true,
        warnings: warnings.to_vec(),
        review_narrative: None,
        assemble_review: None,
    }
}

fn emit_phase(app: &AppHandle, id: &str, phase: u32, label: &str, status: &str) {
    emit(
        app,
        &ProgressEvent::Phase {
            pipeline_id: id.to_string(),
            phase,
            label: label.to_string(),
            status: status.to_string(),
        },
    );
}

fn emit_batch(
    app: &AppHandle,
    id: &str,
    phase: u32,
    idx: u32,
    total: u32,
    files: u32,
    status: &str,
) {
    emit(
        app,
        &ProgressEvent::Batch {
            pipeline_id: id.to_string(),
            phase,
            batch_index: idx,
            total_batches: total,
            file_count: files,
            status: status.to_string(),
        },
    );
}

fn emit_warning(app: &AppHandle, id: &str, phase: u32, message: &str) {
    emit(
        app,
        &ProgressEvent::Warning {
            pipeline_id: id.to_string(),
            phase,
            message: message.to_string(),
        },
    );
}

fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let (year, month, day, hour, min, sec) = epoch_to_ymdhms(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.000Z")
}

fn epoch_to_ymdhms(epoch: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = epoch.div_euclid(86_400);
    let secs_of_day = epoch.rem_euclid(86_400) as u32;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
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

    #[test]
    fn pipeline_registry_assigns_unique_ids() {
        let r = PipelineRegistry::default();
        let a = r.new_id();
        let b = r.new_id();
        assert_ne!(a, b);
        assert!(a.starts_with("pipeline-"));
    }

    #[test]
    fn pipeline_registry_cancel_round_trip() {
        let r = PipelineRegistry::default();
        let id = r.new_id();
        let flag = r.register_cancel(&id);
        assert!(!flag.load(Ordering::SeqCst));
        assert!(r.cancel(&id));
        assert!(flag.load(Ordering::SeqCst));
        r.unregister(&id);
        assert!(!r.cancel(&id));
    }

    #[test]
    fn progress_event_serializes_camel_case() {
        let e = ProgressEvent::Phase {
            pipeline_id: "p1".to_string(),
            phase: 2,
            label: "Scan".to_string(),
            status: "running".to_string(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"pipelineId\":\"p1\""), "got: {json}");
        assert!(json.contains("\"phase\":2"), "got: {json}");
        assert!(!json.contains("pipeline_id"), "snake_case leaked: {json}");
    }

    #[test]
    fn pipeline_config_defaults() {
        let cfg = PipelineConfig::default();
        assert_eq!(cfg.batch_size, 15);
        assert_eq!(cfg.concurrency, 5);
        assert!(cfg.incremental);
        assert!(!cfg.auto_update);
        assert_eq!(cfg.output_language, "en");
    }

    #[test]
    fn progress_event_started_serializes() {
        let e = ProgressEvent::Started {
            pipeline_id: "p2".to_string(),
            repo_name: "demo".to_string(),
            total_phases: 10,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"totalPhases\":10"));
        assert!(json.contains("\"repoName\":\"demo\""));
    }

    #[test]
    fn read_write_config_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let understand_dir = dir.path().join(".understand");
        std::fs::create_dir_all(&understand_dir).unwrap();
        let cfg = PipelineConfig {
            auto_update: true,
            output_language: "zh".to_string(),
            batch_size: 25,
            concurrency: 3,
            incremental: false,
        };
        write_config(&understand_dir, &cfg).unwrap();
        let loaded = read_config(&understand_dir);
        assert_eq!(loaded.output_language, "zh");
        assert_eq!(loaded.batch_size, 25);
        assert!(loaded.auto_update);
        assert!(!loaded.incremental);
    }

    #[test]
    fn read_config_returns_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = read_config(dir.path());
        assert_eq!(loaded.batch_size, 15);
    }

    #[test]
    fn codegraph_kind_mapping_drops_unknown() {
        assert_eq!(codegraph_to_ua_kind("file"), Some("file"));
        assert_eq!(codegraph_to_ua_kind("function"), Some("function"));
        assert_eq!(codegraph_to_ua_kind("method"), Some("function"));
        assert_eq!(codegraph_to_ua_kind("class"), Some("class"));
        assert_eq!(codegraph_to_ua_kind("struct"), Some("class"));
        assert_eq!(codegraph_to_ua_kind("interface"), Some("class"));
        assert_eq!(codegraph_to_ua_kind("type_alias"), Some("class"));
        assert_eq!(codegraph_to_ua_kind("module"), Some("module"));
        assert_eq!(codegraph_to_ua_kind("constant"), Some("concept"));
        assert_eq!(codegraph_to_ua_kind("variable"), Some("concept"));
        assert_eq!(codegraph_to_ua_kind("route"), Some("endpoint"));
        assert_eq!(codegraph_to_ua_kind("component"), Some("service"));
        assert_eq!(codegraph_to_ua_kind("route_component"), None);
        assert_eq!(codegraph_to_ua_kind("import"), Some("module"));
    }

    #[test]
    fn codegraph_edge_mapping_drops_unknown() {
        assert_eq!(codegraph_edge_to_ua("contains"), Some("contains"));
        assert_eq!(codegraph_edge_to_ua("imports"), Some("imports"));
        assert_eq!(codegraph_edge_to_ua("calls"), Some("calls"));
        assert_eq!(codegraph_edge_to_ua("inherits"), Some("inherits"));
        assert_eq!(codegraph_edge_to_ua("implements"), Some("implements"));
        assert_eq!(codegraph_edge_to_ua("exports"), Some("exports"));
        assert_eq!(codegraph_edge_to_ua("references"), None);
        assert_eq!(codegraph_edge_to_ua("related"), None);
    }

    #[test]
    fn knowledge_graph_serializes_camel_case() {
        let node = GraphNode {
            id: "file:src/main.rs".to_string(),
            kind: "file".to_string(),
            name: "main.rs".to_string(),
            file_path: "src/main.rs".to_string(),
            summary: String::new(),
            tags: vec![],
            complexity: "moderate".to_string(),
            location: Some(NodeLocation { start_line: 0, end_line: 0 }),
            language_notes: None,
        };
        let g = KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec!["rust".to_string()],
                frameworks: vec!["Tauri".to_string()],
                description: "Test".to_string(),
                analyzed_at: "2026-06-29T00:00:00.000Z".to_string(),
                git_commit_hash: "deadbeef".to_string(),
            },
            nodes: vec![node],
            edges: vec![],
            layers: vec![],
            tour: vec![],
        };
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains("\"filePath\":\"src/main.rs\""), "got: {json}");
        assert!(json.contains("\"type\":\"file\""), "got: {json}");
        assert!(json.contains("\"frameworks\":[\"Tauri\"]"), "got: {json}");
        assert!(json.contains("\"gitCommitHash\":\"deadbeef\""), "got: {json}");
        assert!(json.contains("\"kind\":\"codebase\""), "got: {json}");
    }

    #[test]
    fn llm_request_spec_converts_provider_strings() {
        for (s, expected) in [
            ("anthropic", LlmProvider::Anthropic),
            ("openai", LlmProvider::Openai),
            ("ollama", LlmProvider::Ollama),
            ("custom", LlmProvider::Custom),
            ("unknown", LlmProvider::Openai), // falls back to OpenAI
        ] {
            let spec = LlmRequestSpec {
                provider: s.to_string(),
                api_key: "k".to_string(),
                model: "m".to_string(),
                base_url: None,
                max_tokens: None,
                temperature: None,
            };
            let req = spec.into_request("sys".to_string(), "user".to_string());
            assert!(
                std::mem::discriminant(&req.provider) == std::mem::discriminant(&expected),
                "provider {s} mapped wrong: got {:?}", req.provider
            );
        }
    }

    #[test]
    fn llm_request_spec_applies_max_tokens_and_temperature_defaults() {
        let spec = LlmRequestSpec {
            provider: "anthropic".to_string(),
            api_key: "k".to_string(),
            model: "claude-3-5-sonnet".to_string(),
            base_url: Some("https://proxy.example.com".to_string()),
            max_tokens: None,
            temperature: None,
        };
        let req = spec.into_request("s".to_string(), "u".to_string());
        assert_eq!(req.max_tokens, 4096);
        assert!((req.temperature - 0.2).abs() < 0.001);
        assert_eq!(req.base_url, Some("https://proxy.example.com".to_string()));
    }

    #[test]
    fn apply_enrichments_mutates_matching_nodes() {
        let mut g = build_empty_graph();
        g.nodes.push(make_node("src/lib.rs", "moderate"));
        g.nodes.push(make_node("src/main.rs", "moderate"));
        let enrichments = vec![
            FileEnrichment {
                path: "src/lib.rs".to_string(),
                summary: "Tiny log lib.".to_string(),
                tags: vec!["logging".to_string()],
                complexity: "simple".to_string(),
                reads_from: vec![],
                writes_to: vec![],
            },
            FileEnrichment {
                path: "src/main.rs".to_string(),
                summary: "Demo entry point.".to_string(),
                tags: vec!["demo".to_string(), "cli".to_string()],
                complexity: "simple".to_string(),
                reads_from: vec![],
                writes_to: vec![],
            },
        ];
        apply_enrichments(&mut g, &enrichments);
        assert_eq!(g.nodes[0].summary, "Tiny log lib.");
        assert_eq!(g.nodes[0].tags, vec!["logging".to_string()]);
        assert_eq!(g.nodes[0].complexity, "simple");
        assert_eq!(g.nodes[1].summary, "Demo entry point.");
        assert_eq!(
            g.nodes[1].tags,
            vec!["demo".to_string(), "cli".to_string()]
        );
    }

    #[test]
    fn apply_enrichments_ignores_unknown_paths() {
        let mut g = build_empty_graph();
        g.nodes.push(make_node("src/lib.rs", "moderate"));
        let enrichments = vec![FileEnrichment {
            path: "src/other.rs".to_string(),
            summary: "X".to_string(),
            tags: vec![],
            complexity: "simple".to_string(),
            reads_from: vec![],
            writes_to: vec![],
        }];
        apply_enrichments(&mut g, &enrichments);
        // Original node unchanged
        assert_eq!(g.nodes[0].summary, "");
        assert_eq!(g.nodes[0].complexity, "moderate");
    }

    #[test]
    fn apply_enrichments_emits_reads_from_edge() {
        let mut g = build_empty_graph();
        g.nodes.push(make_node("src/lib.rs", "moderate"));
        g.nodes.push(make_node("src/config.rs", "moderate"));
        let enrichments = vec![FileEnrichment {
            path: "src/lib.rs".to_string(),
            summary: "X".to_string(),
            tags: vec![],
            complexity: "simple".to_string(),
            reads_from: vec!["src/config.rs".to_string()],
            writes_to: vec![],
        }];
        apply_enrichments(&mut g, &enrichments);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].source, "file:src/lib.rs");
        assert_eq!(g.edges[0].target, "file:src/config.rs");
        assert_eq!(g.edges[0].kind, "reads_from");
    }

    #[test]
    fn apply_enrichments_emits_writes_to_edge() {
        let mut g = build_empty_graph();
        g.nodes.push(make_node("src/saver.rs", "moderate"));
        g.nodes.push(make_node("src/cache.json", "moderate"));
        let enrichments = vec![FileEnrichment {
            path: "src/saver.rs".to_string(),
            summary: "X".to_string(),
            tags: vec![],
            complexity: "simple".to_string(),
            reads_from: vec![],
            writes_to: vec!["src/cache.json".to_string()],
        }];
        apply_enrichments(&mut g, &enrichments);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].kind, "writes_to");
    }

    #[test]
    fn apply_enrichments_drops_unknown_target_for_data_flow() {
        let mut g = build_empty_graph();
        g.nodes.push(make_node("src/lib.rs", "moderate"));
        let enrichments = vec![FileEnrichment {
            path: "src/lib.rs".to_string(),
            summary: "X".to_string(),
            tags: vec![],
            complexity: "simple".to_string(),
            reads_from: vec!["src/missing.rs".to_string()],
            writes_to: vec!["src/also_missing.rs".to_string()],
        }];
        apply_enrichments(&mut g, &enrichments);
        // Neither target exists — no edges
        assert_eq!(g.edges.len(), 0);
    }

    #[test]
    fn apply_enrichments_skips_self_data_flow() {
        let mut g = build_empty_graph();
        g.nodes.push(make_node("src/lib.rs", "moderate"));
        let enrichments = vec![FileEnrichment {
            path: "src/lib.rs".to_string(),
            summary: "X".to_string(),
            tags: vec![],
            complexity: "simple".to_string(),
            reads_from: vec!["src/lib.rs".to_string()],
            writes_to: vec![],
        }];
        apply_enrichments(&mut g, &enrichments);
        assert_eq!(g.edges.len(), 0);
    }

    // -- P1-B: incremental splice tests --

    fn make_graph_with_files(paths: &[&str]) -> KnowledgeGraph {
        let mut g = build_empty_graph();
        for p in paths {
            g.nodes.push(make_node(p, "moderate"));
        }
        g
    }

    #[test]
    fn splice_unchanged_nodes_replaces_prior_versions() {
        // Set up: prior graph on disk with summary "prior" for src/lib.rs;
        // current graph has the same file but no summary yet. Splice
        // should pull the prior node into the current graph (by id).
        let dir = tempdir_for_pipeline();
        let repo_dir = dir.clone();
        let mut prior_graph = make_graph_with_files(&["src/lib.rs", "src/utils.rs"]);
        // Give the prior lib.rs a summary so we can verify the splice
        // actually copied it.
        if let Some(n) = prior_graph.nodes.iter_mut().find(|n| n.file_path == "src/lib.rs") {
            n.summary = "prior summary".to_string();
        }
        std::fs::write(
            &repo_dir.join("knowledge-graph.json"),
            serde_json::to_vec_pretty(&prior_graph).unwrap(),
        )
        .unwrap();

        // Current graph: includes src/lib.rs but with empty summary
        let mut current = make_graph_with_files(&["src/lib.rs", "src/changed.rs"]);
        let spliced =
            splice_unchanged_nodes(&repo_dir, &mut current, &["src/changed.rs".to_string()], &[])
                .unwrap();
        // Two prior nodes (lib.rs, utils.rs) are not in changed_set:
        //   - lib.rs: replaced in place
        //   - utils.rs: appended (was missing from current scan)
        assert_eq!(spliced, 2, "one replaced + one appended");
        // The unchanged src/lib.rs node should now carry the prior summary
        let lib = current
            .nodes
            .iter()
            .find(|n| n.file_path == "src/lib.rs")
            .unwrap();
        assert_eq!(lib.summary, "prior summary");
        // src/utils.rs was missing from the current scan but is appended by splice
        assert!(current.nodes.iter().any(|n| n.file_path == "src/utils.rs"));
    }

    #[test]
    fn splice_unchanged_nodes_skips_changed_paths() {
        // Changed paths are explicitly excluded from splicing.
        let dir = tempdir_for_pipeline();
        let repo_dir = dir.clone();
        let prior_graph = make_graph_with_files(&["src/lib.rs", "src/utils.rs"]);
        std::fs::write(
            &repo_dir.join("knowledge-graph.json"),
            serde_json::to_vec_pretty(&prior_graph).unwrap(),
        )
        .unwrap();

        let mut current = make_graph_with_files(&["src/lib.rs", "src/utils.rs"]);
        // Mark BOTH as changed — splice should not pull anything
        let spliced = splice_unchanged_nodes(
            &repo_dir,
            &mut current,
            &["src/lib.rs".to_string(), "src/utils.rs".to_string()],
            &[],
        )
        .unwrap();
        assert_eq!(spliced, 0);
    }

    #[test]
    fn splice_unchanged_nodes_handles_missing_prior_graph() {
        let dir = tempdir_for_pipeline();
        let mut current = make_graph_with_files(&["src/lib.rs"]);
        let spliced =
            splice_unchanged_nodes(&dir, &mut current, &[], &[]).unwrap();
        assert_eq!(spliced, 0);
    }

    #[test]
    fn splice_unchanged_nodes_appends_when_no_fresh_node() {
        // Prior graph has a node that's not in the current scan
        // (e.g. it was filtered out). Splicing shouldn't add it
        // unless it would have been in the unchanged set.
        let dir = tempdir_for_pipeline();
        let repo_dir = dir.clone();
        let prior_graph = make_graph_with_files(&["src/removed.rs"]);
        std::fs::write(
            &repo_dir.join("knowledge-graph.json"),
            serde_json::to_vec_pretty(&prior_graph).unwrap(),
        )
        .unwrap();

        // Current graph doesn't include src/removed.rs at all.
        // Splicing with changed=[] (so removed.rs is in unchanged
        // set) should append it.
        let mut current = make_graph_with_files(&["src/lib.rs"]);
        let spliced =
            splice_unchanged_nodes(&repo_dir, &mut current, &[], &[]).unwrap();
        assert_eq!(spliced, 1);
        assert!(current.nodes.iter().any(|n| n.file_path == "src/removed.rs"));
    }

    fn tempdir_for_pipeline() -> std::path::PathBuf {
        let unique = format!(
            "codewiki_pipeline_{}_{}",
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

    // -- helpers for the apply_enrichments tests --
    fn make_node(path: &str, complexity: &str) -> GraphNode {
        GraphNode {
            id: format!("file:{path}"),
            kind: "file".to_string(),
            name: path.rsplit_once('/').map(|(_, n)| n).unwrap_or(path).to_string(),
            file_path: path.to_string(),
            summary: String::new(),
            tags: vec![],
            complexity: complexity.to_string(),
            location: None,
            language_notes: None,
        }
    }

    fn build_empty_graph() -> KnowledgeGraph {
        KnowledgeGraph {
            version: "1.0.0".to_string(),
            kind: "codebase".to_string(),
            project: ProjectMeta {
                name: "demo".to_string(),
                languages: vec!["rust".to_string()],
                frameworks: vec![],
                description: String::new(),
                analyzed_at: "2026-06-29T00:00:00.000Z".to_string(),
                git_commit_hash: String::new(),
            },
            nodes: vec![],
            edges: vec![],
            layers: vec![],
            tour: vec![],
        }
    }

    /// End-to-end smoke test: run the full 7-phase pipeline against
    /// a temp project containing a real codegraph DB. Verifies
    /// the final on-disk layout and the camelCase JSON shape the
    /// dashboard consumes.
    #[test]
    fn pipeline_end_to_end_against_real_codegraph() {
        let bin = match which::which("codegraph") {
            Ok(b) => b,
            Err(_) => {
                eprintln!("[pipeline e2e] codegraph not on PATH; skipping");
                return;
            }
        };

        let project = std::env::temp_dir().join(format!(
            "codewiki-pipeline-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&project);
        let repo = project.join("raw").join("code").join("gglog");
        let src = repo.join("src");
        std::fs::create_dir_all(&src).unwrap();
        // Use a small but realistic Cargo manifest so frameworks /
        // description / git detection exercise the real paths.
        std::fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"gglog\"\nversion = \"0.1.0\"\nedition = \"2021\"\ndescription = \"Tiny log lib for testing the pipeline\"\n[dependencies]\ntokio = { version = \"1\" }\nserde = \"1\"\n",
        )
        .unwrap();
        std::fs::write(
            src.join("lib.rs"),
            "//! Tiny log lib.\npub fn log(msg: &str) { println!(\"{}\", msg); }\n",
        )
        .unwrap();
        std::fs::write(
            src.join("tests.rs"),
            "use gglog::log; #[test] fn t() { log(\"hi\"); }\n",
        )
        .unwrap();
        std::fs::write(
            project.join("README.md"),
            "# gglog\n\nA tiny test log library used by the pipeline e2e test.\n",
        )
        .unwrap();
        // Initialise a git repo so `git rev-parse HEAD` returns something.
        let _ = std::process::Command::new("git")
            .arg("-C").arg(&project)
            .arg("init").arg("-q").status();
        let _ = std::process::Command::new("git")
            .arg("-C").arg(&project)
            .args(["config", "user.email", "test@example.com"]).status();
        let _ = std::process::Command::new("git")
            .arg("-C").arg(&project)
            .args(["config", "user.name", "test"]).status();
        let _ = std::process::Command::new("git")
            .arg("-C").arg(&project)
            .args(["add", "."]).status();
        let _ = std::process::Command::new("git")
            .arg("-C").arg(&project)
            .args(["commit", "-q", "-m", "init"]).status();

        // Run codegraph init + index on the real repo.
        let init = std::process::Command::new(&bin)
            .arg("init").arg(&repo).status().expect("codegraph init");
        assert!(init.success(), "codegraph init failed: {:?}", init);
        let idx = std::process::Command::new(&bin)
            .arg("index").arg(&repo).status().expect("codegraph index");
        assert!(idx.success(), "codegraph index failed: {:?}", idx);

        // Build a no-op AppHandle-less invocation of the pipeline
        // orchestrator. We can't construct a real AppHandle in
        // tests, so we exercise `build_graph_via_tree_sitter`
        // directly and assert the same on-disk layout the pipeline
        // would produce.
        let project_path = project.to_string_lossy().to_string();
        let scan = crate::commands::code_wiki_scanner::scan_project_inner(&repo).expect("scan");
        assert!(!scan.files.is_empty(), "scan returned 0 files");
        assert!(scan.git_commit_hash.len() >= 7, "git hash missing");

        let graph = crate::commands::code_wiki_tree_sitter::build_graph_via_tree_sitter(
            &repo,
            "gglog",
            &scan,
        )
        .expect("build ua graph");
        assert!(!graph.nodes.is_empty(), "graph has 0 nodes");
        assert_eq!(graph.kind, "codebase");
        assert_eq!(graph.project.name, "gglog");
        assert!(
            graph.project.description.to_ascii_lowercase().contains("tiny log lib")
                || graph.project.description.to_ascii_lowercase().contains("tiny test log library"),
            "description not extracted from manifest or README: {:?}",
            graph.project.description
        );
        assert!(
            graph.project.frameworks.contains(&"Tokio".to_string())
                || graph.project.frameworks.contains(&"serde".to_string()),
            "frameworks not detected from Cargo.toml: {:?}", graph.project.frameworks
        );

        // M3: run the rest of the pipeline (assemble /
        // architecture / tour / review) before persisting.
        let (graph, asm_report) =
            crate::commands::code_wiki_assembler::assemble(graph);
        assert_eq!(asm_report.edges_dropped, 0, "no edges should dangle");
        let arch = crate::commands::code_wiki_architecture::assign_layers(&graph);
        let mut graph = graph;
        graph.layers = arch.layers.clone();
        let tour = crate::commands::code_wiki_tour::build_tour(&graph);
        graph.tour = tour.steps.clone();
        let _review = crate::commands::code_wiki_reviewer::review_graph(
            &graph,
            &graph.layers,
            &graph.tour,
        );

        // Persist + verify the on-disk shape mirrors what the
        // dashboard server serves.
        let repo_dir = crate::commands::code_wiki::repo_root(&project, "gglog");
        std::fs::create_dir_all(&repo_dir).unwrap();
        let graph_path = repo_dir.join("knowledge-graph.json");
        let json = serde_json::to_vec_pretty(&graph).unwrap();
        std::fs::write(&graph_path, &json).unwrap();
        let read_back: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(read_back["project"]["name"], "gglog");
        assert_eq!(read_back["kind"], "codebase");
        assert!(read_back["nodes"].as_array().unwrap().len() > 0);
        let types: std::collections::HashSet<String> = read_back["nodes"]
            .as_array().unwrap()
            .iter()
            .map(|n| n["type"].as_str().unwrap().to_string())
            .collect();
        assert!(types.contains("file"), "expected file nodes: {types:?}");
        assert!(types.contains("function") || types.contains("class"), "expected symbol nodes: {types:?}");

        // M3: layers and tour should also be populated.
        let layers = read_back["layers"].as_array().unwrap();
        assert!(!layers.is_empty(), "expected non-empty layers array");
        // The test project has lib.rs + tests.rs. Each gets
        // classified into its own layer by the src/<filename>
        // heuristic, so we expect at least 2 layers covering
        // both files. Just assert every layer is well-formed.
        for layer in layers {
            assert!(!layer["id"].as_str().unwrap().is_empty());
            assert!(!layer["name"].as_str().unwrap().is_empty());
            assert!(layer["nodeIds"].as_array().unwrap().len() > 0);
        }

        let tour = read_back["tour"].as_array().unwrap();
        assert!(!tour.is_empty(), "expected non-empty tour array");
        assert_eq!(tour[0]["title"], "Project entry point");
        assert!(tour[0]["nodeIds"].as_array().unwrap().len() > 0);

        let _ = std::fs::remove_dir_all(&project);
        eprintln!("[pipeline e2e] all checks passed");
    }
}
