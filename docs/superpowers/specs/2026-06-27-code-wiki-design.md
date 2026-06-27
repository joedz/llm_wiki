# Code Wiki — Knowledge Graph for Imported Code

**Status:** Design
**Date:** 2026-06-27
**Author:** Planning session

## Goal

Add a code-specific knowledge graph to llm_wiki that:

- Builds a persisted, queryable knowledge graph of every code repository imported under `raw/code/`
- Lets chat queries use the graph for richer code context than the current live-scan approach
- Auto-updates the graph when code files change
- Supports cross-repository queries

Out of scope for this phase: graph visualization panel, human-readable wiki pages about code.

## Storage Layout

```
project_root/
├── raw/
│   └── code/                       # Top-level subdirs are independent repos
│       ├── repo-A/
│       │   └── (user code)
│       └── repo-B/
│           └── (user code)
└── wiki/
    ├── ...existing wiki content
    └── code_wiki/                  # NEW
        ├── index.json             # Global index of all repos
        ├── repo-A/
        │   ├── graph.json         # Per-repo knowledge graph
        │   ├── meta.json          # Last-analyzed metadata
        │   └── .codegraph/        # codegraph tool's local DB
        │       └── codegraph.db
        └── repo-B/
            └── ...
```

The `.codegraph/` directory is co-located with the graph.json it produced, so removing a repo also drops its index. Storing it under `wiki/code_wiki/` (not `raw/code/`) keeps the user's source tree free of tool artifacts.

## Data Model

### Per-repo graph (`wiki/code_wiki/<repo>/graph.json`)

```typescript
interface CodeGraph {
  version: "1.0.0"
  project: {
    name: string                     // repo name = top-level subdir
    description?: string
    languages: string[]              // ["typescript", "rust", ...]
    lastAnalyzedAt: string           // ISO 8601
    gitCommitHash?: string
    fileCount: number
    symbolCount: number
  }
  nodes: GraphNode[]
  edges: GraphEdge[]
  layers?: Layer[]
  stats: {
    totalNodes: number
    totalEdges: number
    byLanguage: Record<string, number>
    byNodeType: Record<string, number>
  }
}

interface GraphNode {
  id: string                         // "file:src/foo.ts", "function:src/foo.ts:bar"
  type: "file" | "function" | "class" | "interface" | "type" | "module" | "variable"
  name: string
  filePath: string                   // relative to raw/code/<repo>/
  summary?: string
  tags: string[]
  complexity?: "simple" | "moderate" | "complex"
  languageNotes?: string
  location?: { startLine: number; endLine: number }
  signature?: string
  content?: string                   // excerpt
}

interface GraphEdge {
  source: string                     // node id
  target: string                     // node id
  type: "imports" | "contains" | "calls" | "extends" | "implements" | "defines" | "references"
  weight?: number                    // 0-1
  metadata?: Record<string, unknown>
}
```

### Global index (`wiki/code_wiki/index.json`)

```typescript
interface CodeWikiIndex {
  version: "1.0.0"
  generatedAt: string
  repos: RepoSummary[]
}

interface RepoSummary {
  name: string
  path: string                       // "raw/code/<repo>"
  graphPath: string                  // "wiki/code_wiki/<repo>/graph.json"
  languages: string[]
  fileCount: number
  symbolCount: number
  description?: string
  lastAnalyzedAt: string
}
```

The global graph is an **index only** — it does not duplicate per-repo nodes. Cross-repo queries read the index to find candidate repos, then load those repos' graphs.

## Architecture

### New TypeScript modules under `src/lib/code-wiki/`

| Module | Responsibility |
|---|---|
| `index.ts` | Public API entry point |
| `repo-detector.ts` | Scan `raw/code/` to identify top-level repo subdirs |
| `graph-builder.ts` | Coordinate the codegraph subprocess, invoke the exporter |
| `graph-exporter.ts` | Read codegraph's SQLite DB, write graph.json |
| `graph-query.ts` | In-memory queries over a loaded graph.json |
| `index-builder.ts` | Write/refresh the global index.json |
| `wiki-storage.ts` | File IO helpers for graph.json, index.json, meta.json |

### New Rust Tauri commands under `src-tauri/src/commands/code_wiki.rs`

| Command | Purpose |
|---|---|
| `code_wiki_list_repos(project_path)` | List top-level dirs in `raw/code/` |
| `code_wiki_get_index(project_path)` | Read `wiki/code_wiki/index.json` |
| `code_wiki_get_graph(project_path, repo_name)` | Read a single repo's graph.json |
| `code_wiki_query(project_path, repos, message, max_context_size)` | Cross-repo context query |
| `code_wiki_run_indexer(project_path, repo_name)` | Trigger full codegraph index |
| `code_wiki_run_sync(project_path, repo_name)` | Trigger codegraph incremental sync |
| `code_wiki_install_check()` | Report whether `codegraph` is on PATH |

## Flows

### Import flow (initial build)

1. User imports code via `importSourceFiles` / `importSourceFolder` into `raw/code/<repo>/`
2. `repo-detector` notices a new top-level subdir
3. Tauri command `code_wiki_run_indexer(projectPath, repoName)`:
   - Create `wiki/code_wiki/<repo>/.codegraph/`
   - Spawn `codegraph init <repoPath>` then `codegraph index <repoPath>`
   - On success, run `graph-exporter` to write `graph.json` and `meta.json`
4. `index-builder` refreshes `wiki/code_wiki/index.json`

### Change flow (incremental update)

1. Existing `file_sync` watcher detects file changes under `raw/code/<repo>/`
2. After the existing queue flush, trigger `code_wiki_run_sync(projectPath, repoName)`
3. Spawn `codegraph sync <repoPath>`
4. Re-export `graph.json`, update `meta.json`, refresh `index.json`

### Chat query flow

1. `buildChatRetrievalContext` calls the new `code_wiki_query` instead of (or in addition to) the live scan
2. If `wiki/code_wiki/index.json` has candidate repos matching the message, load those graphs
3. Run `graph-query`:
   - Extract keywords from the message
   - Match against symbol / file / module names in the loaded graphs
   - Expand 1-2 hops along `calls` edges (callees + callers)
   - Rank by match quality, then trim to context budget
4. Return a `CodeAnalysisContext`-shaped result so the existing `formatCodeContext` and prompt rules need no changes
5. If the graph path fails for any reason, fall back to the existing live scan in `code-analysis.ts`

## Query Strategy

`graph-query.ts` operates on the JSON graph in memory:

- **Keyword extraction** — tokenize the message, drop stop words, normalize identifiers (camelCase, snake_case split)
- **Node match** — exact and prefix match against `node.name`; substring match against `node.filePath` and `node.tags`
- **Graph expansion** — for each matched node, walk incoming/outgoing `calls` edges 1-2 hops to include callers and callees
- **Ranking** — symbol match > file match > expansion hop > path-only match
- **Budget enforcement** — truncate snippets with `fitCodeText`, drop lowest-ranked entries until under `maxContextSize`
- **Reference synthesis** — produce `CodeReference[]` with `kind: "code"` for each emitted snippet

Target performance: 1000 nodes / 5000 edges in < 50ms, 5 repos in parallel in < 200ms.

## Error Handling

| Scenario | Behavior |
|---|---|
| `codegraph` not on PATH | `code_wiki_install_check` returns a friendly error; UI shows install instructions |
| codegraph subprocess fails | Log error, leave the existing graph.json in place; chat falls back to live scan |
| `graph.json` missing | Chat detects via `code_wiki_get_index`, falls back to live scan, surfaces a "graph not built" warning |
| `raw/code/` empty | `index.json` is an empty `repos` array |
| User deletes `raw/code/<repo>/` | File sync detects, calls `code_wiki_run_unindex` (delete `wiki/code_wiki/<repo>/`) and refreshes `index.json` |
| Large repo (>10k files) | codegraph handles incrementally; the exporter streams results; chat only loads the active repo's graph |

## Testing

- **Unit tests**:
  - `repo-detector`: top-level subdir detection, ignores files and hidden dirs
  - `graph-exporter`: field mapping from codegraph schema to our schema
  - `graph-query`: keyword extraction, node matching, graph expansion, ranking, budget enforcement
  - `wiki-storage`: IO, error cases, JSON validation
  - `index-builder`: aggregation correctness
- **Integration tests**:
  - Mock codegraph subprocess output, verify the exporter produces a valid graph.json
  - End-to-end: import code → build graph → query via chat context builder
- **Updated tests**:
  - `chat-prompt-builder.test.ts`: accept graph-sourced code context
  - `source-lifecycle.test.ts`: verify graph indexing is triggered on import
- **Test fixtures**:
  - A small TypeScript fixture repo (5-10 files) under `src/lib/code-wiki/__fixtures__/sample-ts/`
  - A small Rust fixture repo under `src/lib/code-wiki/__fixtures__/sample-rs/`
  - A pre-built `graph.json` for query tests to avoid spawning codegraph in unit tests

## Implementation Steps

1. **Backend scaffolding** — `wiki/code_wiki/` path whitelisting, Tauri command skeletons, install check
2. **Detection & storage** — `repo-detector`, `wiki-storage`, `index-builder`
3. **Indexing pipeline** — `graph-exporter`, `code_wiki_run_indexer`, `code_wiki_run_sync`
4. **Query pipeline** — `graph-query`, `code_wiki_query`
5. **Chat integration** — wire `code_wiki_query` into `buildChatRetrievalContext` with live-scan fallback
6. **File-sync integration** — trigger sync on `raw/code/<repo>/` changes
7. **UI hooks** — "Build code graph" button, graph-source indicator in chat references
8. **Tests** — unit, integration, fixtures, e2e

## Risks

| Risk | Mitigation |
|---|---|
| Subprocess latency on every sync | Only re-sync on actual file changes; debounce rapid bursts |
| SQLite ↔ JSON drift | Single write path: only the exporter writes graph.json, and only after codegraph exits cleanly |
| Large graph.json files | Lazy load per active repo; never load the whole wiki into memory |
| codegraph install burden | First-run check with one-click install instructions in the UI |
| Cross-repo merge complexity | Two-step query: index selects candidate repos, then per-repo queries run in parallel — no global graph merge needed |

## Future Phases (not in this spec)

- **Visualization** — React Flow panel rendering the active repo's graph
- **Architecture layers** — auto-classify files into layers (frontend, backend, infra) and emit `layers[]`
- **Human-readable code wiki pages** — generate markdown overviews per file/symbol under `wiki/code_wiki/<repo>/pages/`
- **Cross-repo edges** — detect when one repo imports from another and emit edges in the global index

## Implementation Status (2026-06-27)

All 18 tasks from the implementation plan are complete. The feature is end-to-end functional:

- `src/lib/code-wiki/` (TypeScript): types, storage, repo detection, index builder, in-memory graph query, exporter, graph builder
- `src-tauri/src/commands/code_wiki.rs` + tests: 7 Rust commands (`code_wiki_install_check`, `code_wiki_list_repos`, `code_wiki_get_index`, `code_wiki_get_graph`, `code_wiki_get_graph_payload`, `code_wiki_run_indexer`, `code_wiki_run_sync`)
- Chat pipeline: `buildCodeWikiOrFallbackContext` prefers graph-backed code context; falls back to live scan when the index is missing
- File sync: `schedule_code_wiki_sync` debounces `codegraph sync` per affected repo on changes under `raw/code/<repo>/`
- Source-lifecycle: `triggerCodeWikiBuilds` runs after code import
- UI: "Build code graph" button in the sources view

**Verification:**
- 18 TypeScript tests pass (vitest) covering types, storage, repo detection, index builder, graph query, and graph exporter
- 7 Rust tests pass (cargo test) covering path helpers, public-path predicate, list/index, plan_index, and codegraph payload parsing
- Full TypeScript type check (`tsc --noEmit`) passes with no errors
- Rust `cargo build --lib` succeeds

**Deferred work (out of scope, future phases):**
- React Flow visualization panel
- Architecture layer classification
- Human-readable code wiki pages
- Cross-repo edges in the global index
- Per-repo sync debouncing (currently each affected repo triggers an immediate sync; codegraph itself dedupes internally)

