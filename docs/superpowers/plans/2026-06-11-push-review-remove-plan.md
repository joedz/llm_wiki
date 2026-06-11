# Push Review - 移除与阅览功能实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为已审批/已拒绝的推送审阅条例增加「移除」和「阅览」按钮，实现彻底删除并写日志、弹窗查看内容。

**Architecture:** 在现有 push-review 架构基础上，新增 bridge 层 remove 事件监听 + store 方法 + 日志持久化 + UI 按钮。复用现有 `saveQueue` 机制，删除后 items 减少自然不同步到 json。

**Tech Stack:** TypeScript, Zustand, Tauri events, React

---

## 文件结构

| 文件 | 变更类型 |
|------|----------|
| `src/stores/push-review-store.ts` | Modify |
| `src/lib/push-review.ts` | Modify |
| `src/lib/push-review-bridge.ts` | Modify |
| `src/components/push-review/push-review-card.tsx` | Modify |
| `src/components/push-review/push-review-modal.tsx` | Modify |

---

### Task 1: Store - 新增 removeItem 方法

**Files:**
- Modify: `src/stores/push-review-store.ts:18-79`

- [ ] **Step 1: 新增 removeItem 方法到 PushReviewState interface**

在 `interface PushReviewState` 中添加：
```typescript
removeItem: (id: string) => void
```

- [ ] **Step 2: 实现 removeItem 方法**

在 `export const usePushReviewStore = create<PushReviewState>((set) => ({` 的 set 对象中添加：
```typescript
removeItem: (id) =>
  set((state) => ({
    items: state.items.filter((item) => item.id !== id),
  })),
```

- [ ] **Step 3: 确认文件最终内容**

检查 `push-review-store.ts` 确认新增方法存在且无语法错误。

---

### Task 2: 日志持久化 - push-review-log.json

**Files:**
- Modify: `src/lib/push-review.ts:1-92`

- [ ] **Step 1: 添加日志文件路径函数**

在 `function queueFilePath` 后添加：
```typescript
function removeLogPath(projectPath: string): string {
  return `${normalizePath(projectPath)}/.llm-wiki/push-review-log.json`
}
```

- [ ] **Step 2: 添加 RemoveLogEntry interface**

在文件顶部添加：
```typescript
interface RemoveLogEntry {
  id: string
  path: string
  removedAt: number
  removedBy?: string
}
```

- [ ] **Step 3: 添加 appendRemoveLog 函数**

在文件末尾添加：
```typescript
export async function appendRemoveLog(
  projectPath: string,
  entry: RemoveLogEntry,
): Promise<void> {
  try {
    const logFile = removeLogPath(projectPath)
    let existing: { version: number; entries: RemoveLogEntry[] } = { version: 1, entries: [] }
    if (await fileExists(logFile)) {
      const raw = await readFile(logFile)
      existing = JSON.parse(raw) as typeof existing
    }
    existing.entries.push(entry)
    await writeFile(logFile, JSON.stringify(existing, null, 2))
  } catch {
    console.error("Failed to write remove log", entry)
  }
}
```

- [ ] **Step 4: 确认文件最终内容**

检查 `push-review.ts` 确认新增函数存在且无语法错误。

---

### Task 3: Bridge 层 - push-review:remove 事件监听

**Files:**
- Modify: `src/lib/push-review-bridge.ts:1-101`

- [ ] **Step 1: 添加 PushReviewRemovePayload interface**

在 `interface PushReviewUpdatePayload` 后添加：
```typescript
interface PushReviewRemovePayload {
  id: string
}
```

- [ ] **Step 2: 添加 handlePushReviewRemove 函数**

在 `async function handlePushReviewUpdate` 后添加：
```typescript
async function handlePushReviewRemove(payload: PushReviewRemovePayload): Promise<void> {
  const project = useWikiStore.getState().project
  const items = usePushReviewStore.getState().items
  const item = items.find((i) => i.id === payload.id)
  if (!item) return

  usePushReviewStore.getState().removeItem(payload.id)

  if (project) {
    await appendRemoveLog(project.path, {
      id: payload.id,
      path: item.path,
      removedAt: Date.now(),
    })
  }
}
```

- [ ] **Step 3: 在 ensurePushReviewBridge 中添加事件监听**

在 `unlistenUpdate` 监听后添加：
```typescript
const unlistenRemove = await listen<PushReviewRemovePayload>("push-review:remove", (event) => {
  void handlePushReviewRemove(event.payload)
})
```

- [ ] **Step 4: 在 return 的 cleanup 函数中调用 unlistenRemove**

```typescript
return () => {
  unlistenSubmit()
  unlistenGetQueue()
  unlistenApprove()
  unlistenReject()
  unlistenUpdate()
  unlistenRemove()
}
```

- [ ] **Step 5: 确认文件最终内容**

检查 `push-review-bridge.ts` 确认新增函数和监听存在且无语法错误。

---

### Task 4: UI - PushReviewCard 增加按钮

**Files:**
- Modify: `src/components/push-review/push-review-card.tsx:1-134`

- [ ] **Step 1: 更新 PushReviewCardProps 接口**

```typescript
interface PushReviewCardProps {
  item: PushQueueItem
  onApprove: (id: string) => void
  onReject: (id: string) => void
  onEdit: (id: string) => void
  onAddNotes: (id: string, notes: string) => void
  onRemove: (id: string) => void        // 新增
  onView: (id: string) => void          // 新增
}
```

- [ ] **Step 2: 从 Zustand 获取 item改为直接使用 props.item**

在 `PushReviewCard` 函数中，直接使用 `item` 而不是重新从 store 查找。

- [ ] **Step 3: 在 status === "pending" 的条件块外添加新按钮区域**

在 `{item.status === "pending" && (` 后面的 buttons 块之后，添加：
```typescript
{item.status !== "pending" && (
  <div className="flex flex-wrap gap-1.5">
    <Button
      variant="outline"
      size="sm"
      className="h-7 text-xs gap-1"
      onClick={() => onView(item.id)}
    >
      <FileText className="h-3 w-3" />
      {t("pushReview.view")}
    </Button>
    <Button
      variant="outline"
      size="sm"
      className="h-7 text-xs gap-1"
      onClick={() => onRemove(item.id)}
    >
      <Trash2 className="h-3 w-3" />
      {t("pushReview.remove")}
    </Button>
  </div>
)}
```

- [ ] **Step 4: 导入 Trash2 和 FileText 图标**

更新 import：
```typescript
import { Check, X, Pencil, MessageSquare, FileText, Trash2 } from "lucide-react"
```

- [ ] **Step 5: 确认文件最终内容**

检查 `push-review-card.tsx` 确认新增 props 和按钮区域存在且无语法错误。

---

### Task 5: UI - ReviewModal 支持只读模式

**Files:**
- Modify: `src/components/push-review/push-review-modal.tsx:1-83`

- [ ] **Step 1: 更新 PushReviewModalProps 接口**

```typescript
interface PushReviewModalProps {
  item: PushQueueItem | null
  onSave: (id: string, reviewNotes: string) => void
  onCancel: () => void
  isOpen: boolean
  readOnly?: boolean  // 新增
}
```

- [ ] **Step 2: 更新 DialogFooter**

```typescript
<DialogFooter>
  <Button variant="outline" onClick={onCancel}>
    {t("pushReview.close")}
  </Button>
  {!readOnly && (
    <Button onClick={() => {
      if (item) {
        onSave(item.id, reviewNotes)
      }
    }}>
      {t("pushReview.save")}
    </Button>
  )}
</DialogFooter>
```

- [ ] **Step 3: 确认文件最终内容**

检查 `push-review-modal.tsx` 确认只读模式支持存在且无语法错误。

---

### Task 6: UI - 父组件连接 onRemove/onView

**Files:**
- Modify: 调用 PushReviewCard 和 PushReviewModal 的父组件（需先确认文件路径）

- [ ] **Step 1: 确认父组件文件**

使用 codegraph 或 grep 搜索调用 `PushReviewCard` 的组件。

- [ ] **Step 2: 添加 onRemove 和 onView 处理函数**

在父组件中添加：
```typescript
const handleRemove = (id: string) => {
  // 调用 Rust 端的 remove 事件
  emit("push-review:remove", { id })
}

const handleView = (id: string) => {
  // 找到对应 item 并打开 Modal
  const item = usePushReviewStore.getState().items.find((i) => i.id === id)
  if (item) {
    setViewModalItem(item)
    setViewModalOpen(true)
  }
}
```

- [ ] **Step 3: 确认文件最终内容**

检查父组件确认新增 props 和处理函数存在且无语法错误。

---

## Self-Review Checklist

1. **Spec coverage:** 所有 spec需求都有对应 task 实现
2. **Placeholder scan:** 无 TBD/TODO/未完成内容
3. **Type consistency:** 方法名和签名一致（removeItem vs remove，onRemove vs handleRemove）