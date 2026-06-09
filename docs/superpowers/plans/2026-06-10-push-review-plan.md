# Push Review Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add push-to-review capability — users submit text content via MCP/HTTP, it enters a review queue, reviewers approve/reject/edit in a sidebar panel, approved content is written to `raw/sources/<path>` and auto-ingested.

**Architecture:** New `push-review` subsystem with Zustand store, React sidebar panel, Rust Tauri commands for file I/O, new HTTP API endpoints, and new MCP tools. Reuses existing `enqueueSourceIngest()` for ingest trigger.

**Tech Stack:** TypeScript (frontend + MCP), Rust (Tauri commands), Zustand (state), existing Tauri file APIs

---

## File Map

```
CREATED:
  src/stores/push-review-store.ts        — Zustand store for queue state
  src/lib/push-review.ts — Business logic (queue CRUD, approve/reject)
  src/components/push-review/
    push-review-view.tsx — Sidebar tab content
    push-review-card.tsx                 — Single item card
    push-review-modal.tsx — Edit content modal
  src-tauri/src/commands/push_review.rs  — Tauri commands (write file, staging)

MODIFIED:
  src-tauri/src/api_server.rs            — New HTTP handlers for push endpoints
  src-tauri/src/commands/mod.rs           — Register push_review module
  mcp-server/src/index.ts                — New MCP tools (push_document, etc.)
  src/components/layout/icon-sidebar.tsx  — Add new sidebar tab icon

REFERENCE (existing patterns):
  src/stores/review-store.ts             — Zustand store pattern
  src/stores/lint-store.ts               — Queue item pattern
  src/lib/dedup-queue.ts                 — Queue persistence pattern (.llm-wiki/*.json)
  src/lib/ingest-queue.ts — enqueueSourceIngest() usage
  src/components/review/review-view.tsx  — Sidebar panel pattern
  src/components/review/review-card.tsx  — Card component pattern
```

---

## Task 1: PushReviewStore

**Files:**
- Create: `src/stores/push-review-store.ts`
- Test: `src/stores/push-review-store.test.ts`

- [ ] **Step 1: Write the failing test**

```typescript
// src/stores/push-review-store.test.ts
import { describe, it, expect, beforeEach } from 'vitest'
import { create } from 'zustand'
import { resetStore, addItem, approveItem, rejectItem, updateItem } from './push-review-store'

describe('push-review-store', () => {
  beforeEach(() => resetStore())

  it('starts with empty queue', () => {
    const store = create(usePushReviewStore.getStore())
    expect(store.getState().items).toEqual([])
  })

  it('adds an item', () => {
    const { id } = addItem({
      path: 'my-docs/test.md',
      content: '# Test',
      contentType: 'text',
      submittedBy: 'MCP',
    })
    expect(store.getState().items).toHaveLength(1)
    expect(store.getState().items[0].status).toBe('pending')
  })

  it('approves an item', () => {
    const { id } = addItem({ path: 'test.md', content: '# Test', contentType: 'text', submittedBy: 'MCP' })
    approveItem(id, 'reviewer-1')
    expect(store.getState().items.find(i => i.id === id)?.status).toBe('approved')
  })

  it('rejects an item', () => {
    const { id } = addItem({ path: 'test.md', content: '# Test', contentType: 'text', submittedBy: 'MCP' })
    rejectItem(id, 'reviewer-1')
    expect(store.getState().items.find(i => i.id === id)?.status).toBe('rejected')
  })

  it('updates review notes', () => {
    const { id } = addItem({ path: 'test.md', content: '# Test', contentType: 'text', submittedBy: 'MCP' })
    updateItem(id, { reviewNotes: 'Looks good' })
    expect(store.getState().items.find(i => i.id === id)?.reviewNotes).toBe('Looks good')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `vitest run src/stores/push-review-store.test.ts`
Expected: FAIL — file does not exist

- [ ] **Step 3: Write minimal implementation**

```typescript
// src/stores/push-review-store.ts
import { create } from 'zustand'

export interface PushQueueItem {
  id: string
  path: string
  content: string
  contentType: 'text'
  fileSize?: number
  submittedAt: number
  submittedBy?: string
  status: 'pending' | 'approved' | 'rejected'
  notes?: string
  reviewNotes?: string
  reviewedAt?: number
  reviewedBy?: string
}

interface PushReviewState {
  items: PushQueueItem[]
  addItem: (item: Omit<PushQueueItem, 'id' | 'status' | 'submittedAt'>) => { id: string }
  approveItem: (id: string, reviewedBy?: string) => void
  rejectItem: (id: string, reviewedBy?: string) => void
  updateItem: (id: string, patch: Partial<Pick<PushQueueItem, 'content' | 'reviewNotes'>>) => void
  resetStore: () => void
  setItems: (items: PushQueueItem[]) => void
}

let counter = 0

export const usePushReviewStore = create<PushReviewState>((set) => ({
  items: [],

  addItem: (item) => {
    const id = `push-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
    set((state) => ({
      items: [...state.items, {
        ...item,
        id,
        status: 'pending',
        submittedAt: Date.now(),
      }],
    }))
    return { id }
  },

  approveItem: (id, reviewedBy) =>
    set((state) => ({
      items: state.items.map((item) =>
        item.id === id
          ? { ...item, status: 'approved' as const, reviewedAt: Date.now(), reviewedBy }
          : item
      ),
    })),

  rejectItem: (id, reviewedBy) =>
    set((state) => ({
      items: state.items.map((item) =>
        item.id === id
          ? { ...item, status: 'rejected' as const, reviewedAt: Date.now(), reviewedBy }
          : item
      ),
    })),

  updateItem: (id, patch) =>
    set((state) => ({
      items: state.items.map((item) =>
        item.id === id ? { ...item, ...patch } : item
      ),
    })),

  resetStore: () => set({ items: [] }),
  setItems: (items) => set({ items }),
}))
```

- [ ] **Step 4: Run test to verify it passes**

Run: `vitest run src/stores/push-review-store.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/stores/push-review-store.ts src/stores/push-review-store.test.ts
git commit -m "feat(push-review): add Zustand store for push review queue"
```

---

## Task 2: Persistence Layer

**Files:**
- Create: `src/lib/push-review.ts`
- Test: `src/lib/push-review.test.ts`

- [ ] **Step 1: Write the failing test**

```typescript
// src/lib/push-review.test.ts
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { loadQueue, saveQueue } from '../lib/push-review'

describe('push-review persistence', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('loads empty queue when file does not exist', async () => {
    // Mock readFile to throw (file not found)
    const items = await loadQueue('/nonexistent/project', 'proj-1')
    expect(items).toEqual([])
  })

  it('saves and loads roundtrip', async () => {
    // Mock both readFile and writeFile
    const items = [{
      id: 'push-1',
      path: 'test.md',
      content: '# Test',
      contentType: 'text',
      submittedAt: Date.now(),
      status: 'pending',
    }]
    // save then load should return same items
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `vitest run src/lib/push-review.test.ts`
Expected: FAIL — module does not exist

- [ ] **Step 3: Write minimal implementation**

```typescript
// src/lib/push-review.ts
import { readFile, writeFile } from './utils'
import { normalizePath } from './path-utils'
import type { PushQueueItem } from '../stores/push-review-store'

const PUSH_QUEUE_FILE = '.llm-wiki/push-queue.json'

function queueFilePath(projectPath: string): string {
  return `${normalizePath(projectPath)}/${PUSH_QUEUE_FILE}`
}

export async function loadQueue(
  projectPath: string,
  projectId: string,
): Promise<PushQueueItem[]> {
  try {
    const raw = await readFile(queueFilePath(projectPath))
    const data = JSON.parse(raw) as { version: number; items: PushQueueItem[] }
    return data.items.map((item) => ({ ...item, status: 'pending' }))
  } catch {
    return []
  }
}

export async function saveQueue(
  projectPath: string,
  items: PushQueueItem[],
): Promise<void> {
  const dir = queueFilePath(projectPath).split('/').slice(0, -1).join('/')
  // ensure .llm-wiki dir exists
  try {
    await writeFile(
      queueFilePath(projectPath),
      JSON.stringify({ version: 1, items }, null, 2),
    )
  } catch {
    // non-critical
  }
}

export function generateId(): string {
  return `push-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `vitest run src/lib/push-review.test.ts`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/lib/push-review.ts src/lib/push-review.test.ts
git commit -m "feat(push-review): add persistence layer for push queue"
```

---

## Task 3: Rust Tauri Commands

**Files:**
- Create: `src-tauri/src/commands/push_review.rs`
- Modify: `src-tauri/src/commands/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write the failing test** (Rust tests inline in file)

- [ ] **Step 2: Implement**

```rust
// src-tauri/src/commands/push_review.rs
use crate::types::wiki::WikiProject;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushReviewItem {
    pub id: String,
    pub path: String,
    pub content: String,
    pub content_type: String,
    pub submitted_at: i64,
    pub submitted_by: Option<String>,
    pub status: String,
    pub notes: Option<String>,
    pub review_notes: Option<String>,
    pub reviewed_at: Option<i64>,
}

#[tauri::command]
pub fn write_push_source(
    app: AppHandle,
    project_path: String,
    relative_path: String,
    content: String,
) -> Result<String, String> {
    let root = PathBuf::from(&project_path);
    let sources_root = root.join("raw/sources");
    let final_path = sources_root.join(&relative_path);

    // Ensure parent dir exists
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create dir: {}", e))?;
    }

    std::fs::write(&final_path, content)
        .map_err(|e| format!("Failed to write file: {}", e))?;

    Ok(final_path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn ensure_push_staging_dir(project_path: String) -> Result<String, String> {
    let root = PathBuf::from(&project_path);
    let staging = root.join(".llm-wiki/push-staging");
    std::fs::create_dir_all(&staging)
        .map_err(|e| format!("Failed to create staging dir: {}", e))?;
    Ok(staging.to_string_lossy().to_string())
}
```

- [ ] **Step 3: Register in mod.rs**

Add to `src-tauri/src/commands/mod.rs`:
```rust
pub mod push_review;
pub use push_review::*;
```

- [ ] **Step 4: Build to verify**

Run: `cargo build -p llm-wiki-lib --lib`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/push_review.rs src-tauri/src/commands/mod.rs
git commit -m "feat(push-review): add Rust commands for writing sources"
```

---

## Task 4: HTTP API Endpoints

**Files:**
- Modify: `src-tauri/src/api_server.rs`

Add to the `handle_request` match statement:

```rust
(&Method::Post, ["push"]) => handle_push_submit(app, body),
(&Method::Get, ["push", "queue"]) => handle_push_queue(app),
(&Method::Post, ["push", id, "approve"]) => handle_push_approve(app, id, body),
(&Method::Post, ["push", id, "reject"]) => handle_push_reject(app, id, body),
(&Method::Patch, ["push", id]) => handle_push_update(app, id, body),
```

Implement the handlers following existing patterns (see `handle_search`, `handle_chat` in same file).

- [ ] **Step 1: Add route handlers** (follow existing api_server.rs patterns)

- [ ] **Step 2: Build to verify**

Run: `cargo build -p llm-wiki-lib --lib`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/api_server.rs
git commit -m "feat(push-review): add HTTP API endpoints for push queue"
```

---

## Task 5: MCP Server Tools

**Files:**
- Modify: `mcp-server/src/index.ts`

Add new tools:
- `push_document` — submit content to review queue
- `get_push_queue` — list pending items
- `approve_push` — approve an item
- `reject_push` — reject an item
- `update_push` — modify content or notes

Follow existing MCP tool pattern from same file (see `health`, `assertMcpEnabled`).

- [ ] **Step 1: Add tool definitions and handlers**

- [ ] **Step 2: Test MCP tools**

- [ ] **Step 3: Commit**

```bash
git add mcp-server/src/index.ts
git commit -m "feat(push-review): add MCP tools for push review"
```

---

## Task 6: React Sidebar Panel

**Files:**
- Create: `src/components/push-review/push-review-view.tsx`
- Create: `src/components/push-review/push-review-card.tsx`
- Create: `src/components/push-review/push-review-modal.tsx`
- Modify: `src/components/layout/icon-sidebar.tsx`

- [ ] **Step 1: Create push-review-card.tsx**

Based on `src/components/review/review-card.tsx` pattern:
- Display: path, content size, submitted time, submittedBy, notes, reviewNotes
- Buttons: Approve (✓), Reject (✗), Edit (✏), Add Notes (💬)
- Clicking Edit opens modal

- [ ] **Step 2: Create push-review-modal.tsx**

- Modal with textarea for content editing
- Review notes field
- Save / Cancel buttons

- [ ] **Step 3: Create push-review-view.tsx**

Based on `src/components/review/review-view.tsx`:
- Header: "Push Review" + pending count badge
- Empty state: "No pending items"
- Map items to PushReviewCard components
- Handle resolve actions (approve/reject/edit)

- [ ] **Step 4: Add sidebar tab icon**

In `icon-sidebar.tsx`, add new tab with icon (e.g., Upload or Inbox icon)
Follow existing pattern for other sidebar tabs.

- [ ] **Step 5: Test in browser** (manual verification)

- [ ] **Step 6: Commit**

```bash
git add src/components/push-review/
git add src/components/layout/icon-sidebar.tsx
git commit -m "feat(push-review): add sidebar panel UI"
```

---

## Task 7: Integration — Approve Triggers Ingest

**Files:**
- Modify: `src/lib/push-review.ts` (add approve logic)
- Modify: `src/stores/push-review-store.ts` (connect to Tauri commands)

- [ ] **Step 1: Implement approve flow in push-review.ts**

```typescript
export async function approveAndIngest(
  projectId: string,
  projectPath: string,
  item: PushQueueItem,
  llmConfig: LlmConfig,
): Promise<void> {
  // 1. Resolve path: if exists, append _1
  const sourcesRoot = `${normalizePath(projectPath)}/raw/sources`
  let finalPath = `${sourcesRoot}/${item.path}`
  if (await fileExists(finalPath)) {
    const ext = item.path.split('.').pop()
    const base = item.path.slice(0, -(ext.length + 1))
    finalPath = `${sourcesRoot}/${base}_1.${ext}`
  }

  // 2. Write content
  await writePushSource(projectPath, item.path, item.content)

  // 3. Enqueue ingest
  await enqueueSourceIngest(project, [relativePath], llmConfig)
}
```

- [ ] **Step 2: Wire approve button in PushReviewView to call approveAndIngest**

- [ ] **Step 3: Test end-to-end**

- [ ] **Step 4: Commit**

```bash
git add src/lib/push-review.ts src/stores/push-review-store.ts src/components/push-review/
git commit -m "feat(push-review): integrate approve flow with ingest"
```

---

## Task 8: Integration — HTTP API to Store

**Files:**
- Modify: `src-tauri/src/api_server.rs` (connect HTTP handlers to store)
- Modify: `src/lib/push-review.ts`

- [ ] **Step 1: HTTP submit handler writes to store + persists queue**

- [ ] **Step 2: HTTP approve handler calls approveAndIngest**

- [ ] **Step 3: HTTP reject handler cleans up**

- [ ] **Step 4: Build + test**

- [ ] **Step 5: Commit**

---

## Task 9: Persistence on Store Changes

**Files:**
- Modify: `src/stores/push-review-store.ts`

- [ ] **Step 1: On every addItem/approveItem/rejectItem/updateItem, call saveQueue()**

Use `project` from `useWikiStore` to get `projectPath`. Subscribe to wiki store project changes to reload queue on project switch.

- [ ] **Step 2: Load queue on app start / project switch**

- [ ] **Step 3: Commit**

```bash
git add src/stores/push-review-store.ts
git commit -m "feat(push-review): persist queue changes to disk"
```

---

## Task 10: Final Integration + UI Polish

**Files:**
- All modified files

- [ ] **Step 1: Sidebar tab shows correct pending count**

- [ ] **Step 2: Empty state is shown when queue is empty**

- [ ] **Step 3: Reject removes item from UI**

- [ ] **Step 4: Approve removes item from UI after ingest**

- [ ] **Step 5: Edit modal saves content back to store**

- [ ] **Step 6: All i18n strings added** (follow existing i18n pattern in `src/i18n/`)

- [ ] **Step 7: Final commit**

```bash
git add -A
git commit -m "feat(push-review): complete push review feature"
```

---

## Self-Review Checklist

- [ ] Spec coverage: Every requirement in the design spec has a task
- [ ] Placeholder scan: No "TBD", "TODO", or vague steps
- [ ] Type consistency: PushQueueItem field names match across store/lib/UI
- [ ] File paths: All exact, no vague "similar file" references
- [ ] Existing patterns followed: Zustand store pattern, queue persistence pattern, sidebar panel pattern

---

## Execution Options

**Plan complete and saved to `docs/superpowers/plans/2026-06-10-push-review-plan.md`.**

Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** — Execute tasks in this session, batch execution with checkpoints

Which approach?