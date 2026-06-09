# Push Review — Design Spec

## Context

LLM Wiki currently supports **querying** the local knowledge base. This spec adds the complementary ability to **push** content into the wiki, with a human review gate before the content is正式纳入。

## Goals

- Users can submit content via MCP tool calls or HTTP POST API
- Submitted content enters a **push review queue** before being正式纳入
- Reviewers (any app user) approve/reject/edit/add notes to each item
- On approval: content written to `raw/sources/<submitter-specified-path>` → triggers ingest → generates wiki pages
- On rejection: staging file discarded, item removed from queue

## Architecture Overview

```
User
  │
  ├─ MCP tool: push_document(path, content, contentType, notes?)
  └─ HTTP POST /api/v1/push
          │
          ▼
  push-queue.json (persisted)
          │
          ▼
  PushReviewStore (Zustand, in-memory state)
          │
          ├──────────────────────────────┐
          ▼ ▼
  [Sidebar: PushReviewView]    [API handlers]
          │                              │
          │  approve ────────────────────┼──→ raw/sources/<path>
          │  reject ─────────────────────┼──→ discard staging file
          │  modify ──────────────────────┼──→ update queue item
          │  add notes ───────────────────┴──→ update queue item
          │
          └─────────────────────────────────→ enqueueSourceIngest()
                                                      │
                                                      ▼
                                              wiki pages generated
```

## File Storage Layout

### Pre-approval (staging)

- **Text content**: stored inline in `push-queue.json` as `content` field
- **File content** (PDF/DOCX/etc): stored as binary file at:
  ```
  <project>/.llm-wiki/push-staging/<uuid>.<ext>
  ```

### Post-approval (final)

```
<project>/raw/sources/<submitter-specified-path>
```

- Path is **relative to `raw/sources/`** — user provides e.g. `my-docs/report.md`
- System automatically prepends `raw/sources/` to get final path: `my-docs/report.md` → `<project>/raw/sources/my-docs/report.md`

## API Surface

### HTTP Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1/push` | Submit content to review queue |
| `GET` | `/api/v1/push/queue` | List all pending items |
| `GET` | `/api/v1/push/:id` | Get single item details |
| `PATCH` | `/api/v1/push/:id` | Update content/notes (reviewer modifies) |
| `POST` | `/api/v1/push/:id/approve` | Approve → write to sources, ingest |
| `POST` | `/api/v1/push/:id/reject` | Reject → discard, remove from queue |

### MCP Tools

| Tool | Parameters | Description |
|------|------------|-------------|
| `push_document` | `path`, `content`, `contentType`, `notes?` | Submit text or base64-encoded file content |
| `get_push_queue` | — | List pending items |
| `approve_push` | `id` | Approve an item |
| `reject_push` | `id` | Reject an item |
| `update_push` | `id`, `content?`, `notes?` | Modify content or notes |

## Data Model

### PushQueueItem

```typescript
interface PushQueueItem {
  id: string                     // e.g. "push-<timestamp>-<random>"
  path: string                   // target path relative to raw/sources: "my-docs/report.md"
  content: string               // text content (plain text only for now)
  contentType: "text"           // reserved for future: "file" support
  fileSize?: number              // bytes (for display)
  submittedAt: number            // Unix timestamp ms
  submittedBy?: string           // caller identifier ("MCP" | "HTTP" | "extension")
  status: "pending" | "approved" | "rejected"
  notes?: string                 // submitter's optional notes
  reviewNotes?: string           // reviewer's notes
  reviewedAt?: number
}
```

### Queue Storage

File: `<project>/.llm-wiki/push-queue.json`

```json
{
  "version": 1,
  "items": [PushQueueItem, ...]
}
```

## Component Inventory

| File | Type | Purpose |
|------|------|---------|
| `src/stores/push-review-store.ts` | Zustand Store | In-memory queue state, CRUD operations |
| `src/components/push-review/push-review-view.tsx` | React Component | Sidebar tab content |
| `src/components/push-review/push-review-card.tsx` | React Component | Single review item card |
| `src/components/push-review/push-review-modal.tsx` | React Component | Edit content modal |
| `src/lib/push-review.ts` | Business Logic | Queue management, approve/reject logic |
| `src-tauri/src/commands/push_review.rs` | Rust Commands | Tauri commands for file ops |
| `src-tauri/src/api_server.rs` | Rust | New HTTP handlers for push endpoints |
| `mcp-server/src/index.ts` | TypeScript | New MCP tools |

## UI Design

### Sidebar Tab

- New sidebar icon (e.g., clipboard with checkmark or upload arrow)
- Panel slides in/out same as other sidebar panels (Activity, Research, etc.)
- Header: "Push Review" + pending count badge
- Empty state: "No pending items"
- List of `PushReviewCard` components

### PushReviewCard Layout

```
┌─────────────────────────────────────────┐
│ [icon]  my-docs/report-2024.pdf         │
│         text ·2.3 MB · 3 minutes ago   │
│         Submitted by: MCP │
│                                         │
│ [Preview excerpt or metadata]           │
│                                         │
│ Notes: "Q3 financial summary"            │
│                                         │
│ [✓ Approve] [✗ Reject] [✏ Edit] [💬] │
└─────────────────────────────────────────┘
```

### Edit Modal

- Full-width text area for content editing
- Review notes field
- Save / Cancel buttons

## Interaction Flow

### Submit (MCP / HTTP)

1. Caller invokes `push_document(path, content, contentType, notes?)`
2. If `contentType === "file"` → write binary to `.llm-wiki/push-staging/<uuid>.<ext>`
3. Create `PushQueueItem`, persist to `push-queue.json`
4. Update in-memory store → UI reflects new item

### Approve Flow

1. Reviewer clicks Approve on card
2. Resolve final path: if `raw/sources/<path>` already exists, append `_1` before extension (e.g., `report.md` → `report_1.md`)
3. Write text content directly to `raw/sources/<path>`
4. Call `enqueueSourceIngest()` to trigger ingest
5. Remove item from queue
6. Refresh sources tree, update UI

### Reject Flow

1. Reviewer clicks Reject
2. If staging file exists → delete it
3. Remove from queue
4. Update UI

### Edit + Approve Flow

1. Reviewer clicks Edit → modal opens with current content
2. Edit content, save
3. Item updated in queue
4. Reviewer clicks Approve (same as Approve Flow above)

## Open Questions / TBD

~~1. **Path prefix convention**: Relative path — user provides `my-docs/report.md`, system prepends `raw/sources/`~~ ✅ RESOLVED

~~2. **Binary file handling**: Not in scope for Phase 1. Only plain text supported initially.~~ ✅ RESOLVED

~~3. **Ingest trigger timing**: Auto-trigger ingest on approval~~ ✅ RESOLVED

~~4. **Duplicate path handling**: Auto-rename with `_1.md` suffix when path exists~~ ✅ RESOLVED

## Implementation Priority

1. **Phase 1**: Push submission (MCP tool + HTTP endpoint + queue persistence)
2. **Phase 2**: Approve/reject flow (write to sources, trigger ingest)
3. **Phase 3**: Edit content + review notes
4. **Phase 4**: UI polish (sidebar integration, notifications, etc.)

## Out of Scope

- Multi-user authentication/authorization (all users can review)
- Automatic content modification by LLM before approval
- Push history / audit log beyond queue persistence