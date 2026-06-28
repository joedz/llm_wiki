# Code Wiki — 7-Phase Parity with Understand-Anything

**Status:** Spec (pending sign-off)
**Date:** 2026-06-28
**Author:** Planning session
**Parent doc:** [2026-06-27-code-wiki-design.md](2026-06-27-code-wiki-design.md)

## Goal

Bring the llm_wiki `code-wiki` analysis pipeline to **full 7-phase parity** with Understand-Anything's `/understand` skill, so that the same data the UA dashboard can render (frameworks, layers, tour, summaries, tags, complexity, descriptions) is also produced by our pipeline.

Concretely, after this work is done, opening a repo in our dashboard should show:
- non-empty `project.frameworks` (e.g. `["React", "Tauri", "Vite"]`)
- non-empty `project.description` (LLM-written or README-derived)
- nodes with **LLM-written** `summary`, `tags`, `complexity` (not the placeholder "moderate")
- a non-empty `layers[]` array
- a non-empty `tour[]` array
- `stats` block at the top level

The same `knowledge-graph.json` we already produce becomes a *real* UA-compatible graph instead of a codegraph-shaped stub.

## Non-goals (out of scope)

- Domain graph (`domain-graph.json`) — keep for Phase 4.
- Knowledge graph from wiki articles (`understand-knowledge`) — keep for Phase 4.
- Onboarding guide (`understand-onboard`) — keep for Phase 4.
- Visual changes to the dashboard (we ship the UA SPA as-is).
- Migrating existing `wiki/code_wiki/<repo>/knowledge-graph.json` files in place — the new pipeline will overwrite them when the user re-runs analysis.

## Storage layout (extends existing)

```
project_root/
└── wiki/code_wiki/
    ├── index.json
    └── repo-A/                          # per-repo (unchanged)
        ├── knowledge-graph.json         # final UA KnowledgeGraph (now fully populated)
        ├── meta.json                    # final AnalysisMeta
        ├── config.json                  # NEW: { autoUpdate, outputLanguage }
        └── .understand/                 # NEW: pipeline intermediate (mirrors UA's `.understand-anything/`)
            ├── scan-result.json          # phase 1 output
            ├── batches.json              # phase 1.5 output (batch plan)
            ├── batch-0.json, …, batch-N.json    # phase 2 per-batch LLM output
            ├── assembled-graph.json      # phase 3 output
            ├── layers.json               # phase 4 output
            ├── tour.json                 # phase 5 output
            ├── review.json               # phase 6 output (validation issues + warnings)
            ├── fingerprints.json         # phase 7 fingerprint baseline
            ├── .understandignore         # user's per-repo exclude rules (phase 0.5)
            └── tmp/                      # scratch space (cleared after each phase)
```

The `.understand/` subdirectory is hidden (starts with `.`) and gitignored in user projects. It mirrors UA's `.understand-anything/` layout so anyone porting tooling recognizes it.

## 7-Phase Pipeline

| # | Phase | What it does | Static / LLM | Where it runs |
|---|---|---|---|---|
| 0 | Pre-flight | Resolve `PROJECT_ROOT` (worktree redirect), capture `git rev-parse HEAD`, read `outputLanguage` + `autoUpdate` from `config.json`, merge subdomains, decide full vs incremental | Static | Rust |
| 0.5 | Ignore config | Generate `.understandignore` from `.gitignore` + repo heuristics | Static | Rust |
| 1 | Scan | Enumerate files, assign language + fileCategory, count lines, detect frameworks from manifests, write `scan-result.json` | Mostly static + 1 LLM call (manifest narrative) | Rust + 1 LLM call |
| 1.5 | Batch | Compute batch plan (groups of files for parallel LLM dispatch), preserving cross-batch import edges | Static | Rust |
| 2 | Analyze | Per batch: structural extraction (tree-sitter via `codegraph`) + LLM summary/tags/complexity + cross-batch edges via `neighborMap` | Static extraction + LLM (per batch) | Rust orchestrator + 1 LLM call per batch (5 concurrent) |
| 3 | Assemble review | Merge batch outputs, normalize node IDs, deduplicate, validate | Static | Rust |
| 4 | Architecture | Compute structural patterns (directory groups, adjacency matrix) + LLM layer assignments | Static compute + 1 LLM call | Rust + 1 LLM call |
| 5 | Tour | LLM generates 5-15 guided tour steps aligned with project structure | 1 LLM call | Rust + 1 LLM call |
| 6 | Review | Inline deterministic validation (or LLM `--review` mode) | Static (default) or LLM | Rust |
| 7 | Save | Write `knowledge-graph.json` + `meta.json` + `fingerprints.json`, cleanup intermediate | Static | Rust |

For incremental updates, the scan phase is skipped if `scan-result.json` is cached; only Phase 2 is re-run for changed files (detected via `git diff` between current HEAD and the `gitCommitHash` recorded in `meta.json`).

## LLM integration

**Source of LLM config:** the same `LlmConfig` that powers the chat panel (`useWikiStore.llmConfig`). The analysis pipeline reads the config at run time, so any provider the user has set up (Anthropic, OpenAI, Ollama, etc.) works without code changes.

**Where the LLM call lives:** Rust backend. The TS frontend hands off to a Tauri command that spawns a background tokio task; the task makes HTTP calls to the LLM provider directly (we already use `reqwest` for chat). No Node.js dependency for the LLM phase.

**Batching:** Phase 2 calls the LLM once per batch. Default batch size = 15 files (matches UA's 10-15 range); configurable via `config.json`. Up to 5 batches run concurrently (matches UA).

**Retry policy:** exponential backoff, 3 attempts. Per-batch failures are logged into the final report and the batch is dropped (per UA's "always save partial results" rule). The pipeline never fails the whole run because of one bad batch.

**Token estimation:** before the run, estimate input tokens as `sum(batch.sizeLines / 4)` and surface the estimate in the UI before the user confirms.

**Output schema enforcement:** every LLM phase returns JSON. We parse with `serde_json::Value` first (lenient), then run a strict shape validator. Schema violations fall back to "phase warning" and the LLM is retried once with the schema constraint appended to the prompt.

**Cancelled runs:** the user can click "Cancel" in the progress UI. The Rust task watches an `AtomicBool`; each phase checks at its boundaries, and the LLM call loop checks between batches. A cancelled run writes whatever was completed to disk and emits a final `pipeline:cancelled` event.

## Progress events

The pipeline emits a Tauri event `codewiki-pipeline-progress` on every state change:

```ts
type ProgressEvent =
  | { kind: "started"; repoName: string; totalPhases: 7 }
  | { kind: "phase"; phase: 0 | 0.5 | 1 | 1.5 | 2 | 3 | 4 | 5 | 6 | 7; label: string; status: "running" | "done" | "error"; startedAt: string; finishedAt?: string }
  | { kind: "batch"; phase: 2; batchIndex: number; totalBatches: number; fileCount: number; status: "running" | "done" | "error" }
  | { kind: "token-estimate"; inputTokens: number; outputTokens: number }
  | { kind: "warning"; phase: number; message: string }
  | { kind: "cancelled"; phase: number; partialSaved: boolean }
  | { kind: "done"; finalGraphPath: string; nodeCount: number; edgeCount: number; layerCount: number; tourStepCount: number; warnings: string[] }
```

The TS frontend subscribes and renders a progress bar + phase list + warnings panel + cancel button. The progress UI lives in `CodeWikiView` (not a modal) so the user can keep browsing.

## UI integration (CodeWikiView)

A new button next to `[Rebuild]` and `[Open Dashboard]`:

```
[Rebuild]   [Analyze with LLM ⟳]   [Open Dashboard ↗]   [📋 URL]
```

- `[Rebuild]` — current behavior, codegraph-only, fast (~seconds)
- `[Analyze with LLM ⟳]` — full 7-phase pipeline, slow (minutes for big repos)
- Both write the same `knowledge-graph.json`; the LLM version overwrites with richer content

A persistent progress panel below the repo list shows:
- Current phase + spinner
- Batch progress (when in phase 2): "Analyzing batch 3/12 (15 files)"
- Token estimate before confirm
- Live warnings
- Cancel button

The progress panel survives navigation (it's in the wiki store, not local component state).

## File structure (new code)

```
src-tauri/src/commands/
├── code_wiki.rs                    (existing; add phase 0 / 0.5 helpers)
├── code_wiki_pipeline.rs           NEW: orchestrator, runs all 7 phases, emits progress
├── code_wiki_scanner.rs            NEW: phase 1 — file enumeration, language detection, frameworks from manifests
├── code_wiki_batcher.rs            NEW: phase 1.5 — batch plan computation, cross-batch edge preservation
├── code_wiki_analyzer.rs           NEW: phase 2 — per-batch structural extraction + LLM call
├── code_wiki_assembler.rs          NEW: phase 3 — merge, dedup, normalize
├── code_wiki_architecture.rs       NEW: phase 4 — directory grouping, adjacency, LLM layer assignment
├── code_wiki_tour.rs               NEW: phase 5 — LLM tour generation
├── code_wiki_reviewer.rs           NEW: phase 6 — deterministic validation (--review → LLM)
├── code_wiki_save.rs               NEW: phase 7 — write final files, fingerprints, cleanup
└── code_wiki_dashboard.rs         (existing, unchanged)

src-tauri/src/commands/code_wiki_pipeline_tests.rs    NEW: full pipeline integration test
src-tauri/src/commands/code_wiki_scanner_tests.rs     NEW: scanner unit tests
src-tauri/src/commands/code_wiki_batcher_tests.rs     NEW: batcher unit tests

src/lib/code-wiki/
├── pipeline.ts                     NEW: TS client for the orchestrator (start, subscribe progress, cancel)
├── pipeline-store.ts               NEW: zustand store for progress state
├── scan-result-types.ts            NEW: TypeScript types for the intermediate JSON shapes
├── llm-prompt-templates.ts         NEW: prompt builders for each LLM phase
└── (existing files unchanged)

src/components/code-wiki/
├── code-wiki-view.tsx              (existing; add LLM button + progress panel)
├── pipeline-progress.tsx           NEW: progress bar + phase list + cancel
└── (existing unchanged)
```

## LLM prompt strategy

The prompts are ported from UA's agent `.md` files with the following adjustments:

1. **No "you are an LLM agent" framing** — the Rust code is the agent, the LLM is just the brain.
2. **Input is structured JSON** (passed as a system message), output is JSON we parse — UA's prompts include source code as text; we keep that for Phase 2 but for Phases 4/5 the input is the pre-computed structural data.
3. **Locale support** — we read the project's `outputLanguage` and append the same language directive UA uses (port the 4-language baseline; expand later).
4. **Schema reminders** — every prompt ends with a "respond with valid JSON matching the schema" reminder; the strict validator on Rust side catches failures.

We do **not** ship the 4 baseline language files (`locales/{en,zh,ja,ko}.md`); we just hardcode the directive string per locale for now. Phase 4 work.

## Config file

`wiki/code_wiki/<repo>/config.json` mirrors UA's `.understand-anything/config.json`:

```json
{
  "autoUpdate": false,
  "outputLanguage": "en",
  "batchSize": 15,
  "concurrency": 5,
  "incremental": true
}
```

The user sets these via a new section in the existing Settings → Code Wiki (or via the LLM button's modal). The pipeline reads from here on every run; UA's behavior of persisting `--auto-update` / `--language` is replicated.

## Fingerprint baseline (Phase 7)

Mirrors UA's `build-fingerprints.mjs`. We compute a per-file structural fingerprint (function/class counts, cyclomatic-ish metric, top-level exports) and persist to `fingerprints.json`. Future incremental runs compare current fingerprints against this baseline to classify each changed file as `UNCHANGED` / `STALE` / `STRUCTURAL` (UA's three-class system, see UA `change-classifier.ts`).

For our scope: implement just enough to detect "this file structurally changed → re-run LLM for it" vs "this file is byte-identical → skip". No need for the full 3-class system in Milestone 1.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| LLM call latency makes full analysis feel slow | Show progress + cancel button; user can fall back to "Rebuild" for fast path |
| LLM cost runaway on huge repos | Token estimate up front; user confirms before run; concurrency cap (5 batches) |
| LLM output schema mismatch | Strict validator on Rust side; auto-retry once with schema reminder |
| Pipeline crashes mid-run | Always save partial results (UA's rule); on resume, detect last-completed phase from `.understand/` |
| Different LLM providers return slightly different JSON | Lenient parse first, strict validate second; LLM retried once on failure |
| Worktree redirect (UA's issue #133) | llm_wiki's projects are explicit (user picks a dir), not auto-discovered, so worktree redirect is unnecessary. Skip the worktree logic. |
| Concurrent analyses on the same repo | Lock file at `.understand/.lock`; second run errors with friendly message |

## Edge cases

- **No LLM configured** → button hidden or shows "Configure LLM in Settings" link
- **Empty repo** → phase 1 returns 0 files, pipeline produces an empty graph, dashboard opens fine
- **Repo > 1000 files** → warn user, suggest scoping; pipeline proceeds if confirmed
- **Repo > 10000 files** → require user confirmation; consider adding incremental mode default-on
- **Cancelled mid-phase** → write partial results, mark run as `cancelled` in `meta.json`
- **LLM provider timeout** → batch marked `error`, other batches continue
- **graph.json file watcher in the same project** → pipeline respects existing watch; no special coordination needed

## Implementation milestones

### Milestone 1: Plumbing + scan + batch + save (no LLM)
**Scope:** Phase 0, 0.5, 1, 1.5, 7 only (no LLM). Output is a UA-shape graph with real `project.frameworks` (from manifests) but everything else still static (summary, tags, complexity, layers, tour all empty/placeholder).
**Outcome:** The "Analyze with LLM" button starts working — without the LLM bits, it produces a structurally-correct but content-empty graph. The pipeline emits progress events. Cancel works. Code reviewable end-to-end.
**Effort:** ~3-4 days.

### Milestone 2: LLM-enhanced file analysis
**Scope:** Add Phase 2. Per-batch LLM call for summary + tags + complexity + cross-batch edges. Migrate the `file-analyzer.md` prompt.
**Outcome:** Each node in the graph has a real LLM-written `summary` and `tags` array. `complexity` is informed by the script's metrics + LLM judgment.
**Effort:** ~2-3 days. (Heavy on prompt engineering + LLM reliability testing.)

### Milestone 3: Architecture, tour, review, UI polish
**Scope:** Add Phases 3, 4, 5, 6. Wire the progress UI. Add token-estimate + cancel UI. Add LLM button to settings.
**Outcome:** Full feature parity with UA's `/understand` for code repos. Dashboard shows non-empty layers + tour + LLM-written summaries.
**Effort:** ~3-4 days.

**Total: 8-11 days for full parity.** (UA's `understand` skill took a team ~18 months to build; we have a head start on the prompts and shapes.)

## Migration / compatibility

- Existing `wiki/code_wiki/<repo>/knowledge-graph.json` files (with empty `summary`/etc) are read by the new pipeline as "current graph" — Phase 0 sees the existing file, treats the run as full (not incremental) since no `meta.json` `gitCommitHash` exists, and overwrites.
- The old `graph-builder.ts` / `knowledge-graph-writer.ts` / `code_wiki_run_indexer` Rust commands stay — they remain the "fast path" (no LLM). The new pipeline is a separate command (`code_wiki_run_pipeline`) that calls the same building blocks.
- The `code_wiki_open_dashboard` server needs no changes — it serves whatever `knowledge-graph.json` is on disk.
- `buildCodeWikiForRepo` in `graph-builder.ts` is renamed to `buildGraphForRepoFast` (or aliased) to make the "no LLM" framing clear. A new `analyzeRepoWithLlm` is the LLM entry point.

## Testing strategy

- **Unit tests** per phase module: scanner (file enumeration, language detection, manifest parsing), batcher (batch sizes, cross-batch edges), assembler (dedup, ID normalization), reviewer (schema validation), save (file write, fingerprint compute).
- **Integration test**: full pipeline against a tiny repo (5-10 files). Asserts every intermediate file is written, every progress event is emitted, the final `knowledge-graph.json` is valid UA shape and contains expected node/edge counts.
- **End-to-end test with LLM (gated)**: same as above but with real LLM. Gated on `LLM_E2E=1` env var so CI without LLM access skips.
- **Real-DB e2e** (existing): the fast path still works (regression check).

## Open questions for sign-off

1. **Should we cap the LLM budget per project?** A 10k-file repo with 1k tokens/file ≈ 10M tokens input just for Phase 2 — that's a meaningful bill. We could ask the user up front: "this will cost roughly $X". Or hard-cap at e.g. 100 batches and refuse beyond that.
2. **What happens if the user has no LLM configured?** Two options: (a) button is hidden, only "Rebuild" available; (b) button shows "Configure LLM first" and links to settings. My recommendation: (a), simpler.
3. **Should we save Phase 7 output (final `knowledge-graph.json`) atomically?** UA doesn't, but on crash mid-write you'd lose the graph. My recommendation: write to `knowledge-graph.json.tmp` first, then rename — UA's `meta.json`-after-fingerprints invariant stays.
4. **Should we keep `graph.json` as an alias for `knowledge-graph.json`?** UA never had `graph.json`; we deleted it last week. No need to add back.

## Success criteria

After this is done:

- `[Analyze with LLM]` on a 50-file repo finishes in <2 minutes, populates `frameworks`, `description`, node `summary`/`tags`/`complexity`, `layers[]` (3-5 layers), `tour[]` (5-8 steps)
- Dashboard shows a richer view: layer legend, project description, language lessons on tour stops
- Cancelling mid-run keeps partial results and shows a clear "cancelled" state
- Re-running on the same repo (no changes) is detected as incremental and completes in <5 seconds
- 100% of UA's `/understand` skill phases have an equivalent in our pipeline (modulo the worktree-redirect corner case)

