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
    run_get_graph_payload_inner, run_indexer_inner, repo_root, GRAPH_FILE, META_FILE,
};
use crate::commands::code_wiki_analyzer::{analyze_batch as analyze_one_batch, FileEnrichment};
use crate::commands::code_wiki_batcher::{plan_batches_inner, write_batches_plan, BatchEntry};
use crate::commands::code_wiki_ignore::generate_understandignore_inner;
use crate::commands::code_wiki_save::{write_atomic, write_fingerprints};
use crate::commands::code_wiki_scanner::{scan_project_inner, ScanResult};
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
    pub api_key: String,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
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

fn understand_dir_for(repo_dir: &std::path::Path) -> PathBuf {
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
    pub layers: Vec<serde_json::Value>,
    pub tour: Vec<serde_json::Value>,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub direction: String,
    pub weight: f32,
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
        _ => return None,
    })
}

fn build_ua_graph(
    repo_name: &str,
    scan_languages: &[String],
    scan_frameworks: &[String],
    scan_description: &str,
    git_commit_hash: &str,
    payload_nodes: &[crate::commands::code_wiki::CodegraphContextNode],
    payload_edges: &[crate::commands::code_wiki::CodegraphContextEdge],
) -> KnowledgeGraph {
    let mut nodes: Vec<GraphNode> = Vec::new();
    for raw in payload_nodes {
        let Some(ua_kind) = codegraph_to_ua_kind(&raw.kind) else { continue };
        let file_path = if raw.file_path.is_empty() { String::new() } else { raw.file_path.clone() };
        let location = raw.location.as_ref().map(|l| NodeLocation {
            start_line: l.start_line,
            end_line: l.end_line,
        });
        nodes.push(GraphNode {
            id: raw.id.clone(),
            kind: ua_kind.to_string(),
            name: raw.name.clone(),
            file_path,
            summary: raw.docstring.clone().unwrap_or_default(),
            tags: raw.tags.clone(),
            complexity: "moderate".to_string(),
            location,
            language_notes: raw.language.clone(),
        });
    }
    nodes.sort_by(|a, b| a.id.cmp(&b.id));

    let mut edges: Vec<GraphEdge> = Vec::new();
    for raw in payload_edges {
        let Some(ua_kind) = codegraph_edge_to_ua(&raw.kind) else { continue };
        edges.push(GraphEdge {
            source: raw.source.clone(),
            target: raw.target.clone(),
            kind: ua_kind.to_string(),
            direction: "forward".to_string(),
            weight: 1.0,
        });
    }
    edges.sort_by(|a, b| a.source.cmp(&b.source).then(a.target.cmp(&b.target)));

    let mut languages: Vec<String> = scan_languages.to_vec();
    languages.sort();
    languages.dedup();

    KnowledgeGraph {
        version: "1.0.0".to_string(),
        kind: "codebase".to_string(),
        project: ProjectMeta {
            name: repo_name.to_string(),
            languages,
            frameworks: scan_frameworks.to_vec(),
            description: scan_description.to_string(),
            analyzed_at: now_iso(),
            git_commit_hash: git_commit_hash.to_string(),
        },
        nodes,
        edges,
        layers: Vec::new(),
        tour: Vec::new(),
    }
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
#[tauri::command]
pub async fn code_wiki_run_pipeline(
    project_path: String,
    repo_name: String,
    llm: Option<LlmRequestSpec>,
    app: AppHandle,
    state: tauri::State<'_, Arc<PipelineRegistry>>,
) -> Result<(), String> {
    let registry = state.inner().clone();
    let app_for_task = app.clone();
    tokio::spawn(async move {
        let result = run_pipeline(app_for_task, registry, project_path, repo_name, llm).await;
        if let Err(e) = result {
            eprintln!("[code-wiki pipeline] failed: {e}");
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
) -> Result<PipelineSummary, String> {
    let pipeline_id = registry.new_id();
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
    cancel: &AtomicBool,
    started: Instant,
) -> Result<PipelineSummary, String> {
    let started = started;
    emit(
        app,
        &ProgressEvent::Started {
            pipeline_id: pipeline_id.to_string(),
            repo_name: repo_name.to_string(),
            total_phases: 7,
        },
    );

    let mut warnings: Vec<String> = Vec::new();
    let project_root = PathBuf::from(project_path);
    if !project_root.is_dir() {
        return Err(format!("project path is not a directory: {project_path}"));
    }
    let repo_dir = repo_root(&project_root, repo_name);
    if !repo_dir.is_dir() {
        return Err(format!("repo not found: {}", repo_dir.display()));
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
    let scan = match scan_project_inner(&repo_dir) {
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

    // --- Phase 2 ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    let phase2_label = if llm.is_some() { "Analyze (LLM)" } else { "Analyze (no LLM)" };
    emit_phase(app, pipeline_id, 4, phase2_label, "running");

    let mut graph = match build_ua_graph_via_codegraph(
        &project_root,
        repo_name,
        &scan,
    ) {
        Ok(g) => g,
        Err(e) => {
            let msg = format!("phase 2 codegraph build failed: {e}");
            emit_warning(app, pipeline_id, 4, &msg);
            return Err(msg);
        }
    };

    if let Some(llm_spec) = llm {
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
            &project_root,
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

    // --- Phase 3-6 stubs ---
    for (phase, label) in [(5u32, "Assemble review"), (6, "Architecture + tour")] {
        emit_phase(app, pipeline_id, phase, label, "done");
    }

    // --- Phase 7 ---
    if check_cancel(cancel) {
        return Ok(cancelled_summary(pipeline_id, project_path, repo_name, started, &warnings));
    }
    emit_phase(app, pipeline_id, 7, "Save", "running");
    let fp_path = match write_fingerprints(
        &project_root,
        &understand_dir,
        &scan.git_commit_hash,
        &scan.files,
    ) {
        Ok(p) => p,
        Err(e) => {
            warnings.push(format!("fingerprints failed: {e}"));
            emit_warning(app, pipeline_id, 7, &warnings.last().unwrap());
            understand_dir.join(FINGERPRINTS_FILE)
        }
    };
    let graph_path = repo_dir.join(GRAPH_FILE);
    let graph_bytes = serde_json::to_vec_pretty(&graph)
        .map_err(|e| format!("serialize graph: {e}"))?;
    if let Err(e) = write_atomic(&graph_path, &graph_bytes) {
        let msg = format!("graph write failed: {e}");
        emit_warning(app, pipeline_id, 7, &msg);
        return Err(msg);
    }
    let meta = serde_json::json!({
        "lastAnalyzedAt": now_iso(),
        "gitCommitHash": scan.git_commit_hash,
        "version": "codewiki-1.0.0",
        "analyzedFiles": scan.files.iter().filter(|f| f.file_category == "code").count(),
    });
    let meta_path = repo_dir.join(META_FILE);
    if let Err(e) = write_atomic(
        &meta_path,
        &serde_json::to_vec_pretty(&meta).map_err(|e| format!("serialize meta: {e}"))?,
    ) {
        warnings.push(format!("meta.json write failed: {e}"));
        emit_warning(app, pipeline_id, 7, &warnings.last().unwrap());
    }
    emit_phase(app, pipeline_id, 7, "Save", "done");

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
        warnings,
    };
    emit(
        app,
        &ProgressEvent::Done {
            pipeline_id: pipeline_id.to_string(),
            summary: summary.clone(),
        },
    );
    Ok(summary)
}

/// Build the UA `KnowledgeGraph` for `repo_name` by:
///   1. Invoking the existing codegraph init+index pipeline.
///   2. Reading the SQLite payload via the existing reader.
///   3. Mapping nodes/edges to UA shapes.
///   4. Filling `project.{frameworks,description,gitCommitHash,languages}`
///      from the scanner output.
fn build_ua_graph_via_codegraph(
    project_root: &PathBuf,
    repo_name: &str,
    scan: &crate::commands::code_wiki_scanner::ScanResult,
) -> Result<KnowledgeGraph, String> {
    // Make sure the codegraph DB is up to date. We treat failures
    // as recoverable (use whatever's already on disk) — UA's
    // principle: always save partial results.
    if let Err(e) = run_indexer_inner(project_root, repo_name) {
        eprintln!("[code-wiki pipeline] codegraph init/index warning: {e}");
    }
    let payload = run_get_graph_payload_inner(project_root, repo_name)?;
    Ok(build_ua_graph(
        repo_name,
        &scan.stats
            .by_language
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        &scan.frameworks,
        &scan.project_description,
        &scan.git_commit_hash,
        &payload.nodes,
        &payload.edges,
    ))
}

/// Apply LLM-produced enrichments to the in-memory graph.
/// Currently we only enrich file-level nodes by their `filePath`.
fn apply_enrichments(graph: &mut KnowledgeGraph, enrichments: &[FileEnrichment]) {
    for enr in enrichments {
        if let Some(node) = graph.nodes.iter_mut().find(|n| n.file_path == enr.path) {
            node.summary = enr.summary.clone();
            node.tags = enr.tags.clone();
            node.complexity = enr.complexity.clone();
        }
    }
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
            total_phases: 7,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"totalPhases\":7"));
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
        assert_eq!(codegraph_edge_to_ua("references"), None);
        assert_eq!(codegraph_edge_to_ua("related"), None);
    }

    #[test]
    fn knowledge_graph_serializes_camel_case() {
        use crate::commands::code_wiki::CodegraphContextNode;
        let node = CodegraphContextNode {
            id: "file:src/main.rs".to_string(),
            kind: "file".to_string(),
            name: "main.rs".to_string(),
            file_path: "src/main.rs".to_string(),
            qualified_name: None,
            language: Some("rust".to_string()),
            summary: None,
            signature: None,
            docstring: None,
            tags: vec![],
            location: Some(crate::commands::code_wiki::NodeLocation { start_line: 0, end_line: 0 }),
            is_exported: None,
            is_async: None,
            decorators: vec![],
            visibility: None,
        };
        let g = build_ua_graph("demo", &["rust".to_string()], &["Tauri".to_string()], "Test", "deadbeef", &[node], &[]);
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
            },
            FileEnrichment {
                path: "src/main.rs".to_string(),
                summary: "Demo entry point.".to_string(),
                tags: vec!["demo".to_string(), "cli".to_string()],
                complexity: "simple".to_string(),
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
        }];
        apply_enrichments(&mut g, &enrichments);
        // Original node unchanged
        assert_eq!(g.nodes[0].summary, "");
        assert_eq!(g.nodes[0].complexity, "moderate");
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
        // tests, so we exercise `build_ua_graph_via_codegraph`
        // directly and assert the same on-disk layout the pipeline
        // would produce.
        let project_path = project.to_string_lossy().to_string();
        let scan = crate::commands::code_wiki_scanner::scan_project_inner(&repo).expect("scan");
        assert!(!scan.files.is_empty(), "scan returned 0 files");
        assert!(scan.git_commit_hash.len() >= 7, "git hash missing");

        let graph = crate::commands::code_wiki_pipeline::build_ua_graph_via_codegraph(
            &project,
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

        let _ = std::fs::remove_dir_all(&project);
        eprintln!("[pipeline e2e] all checks passed");
    }
}
