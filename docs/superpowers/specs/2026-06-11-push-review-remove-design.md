# Push Review -移除与阅览功能设计

## 概述

为已审批（approved）或已拒绝（rejected）的推送审阅条例在面板上增加两个按钮：移除（彻底删除并写日志）和阅览（弹窗查看内容）。

## 变更范围

| 文件 | 变更类型 |
|------|----------|
| `src/stores/push-review-store.ts` | 修改 |
| `src/lib/push-review.ts` | 修改 |
| `src/lib/push-review-bridge.ts` | 修改 |
| `src/components/push-review/push-review-card.tsx` | 修改 |
| `src/components/push-review/push-review-modal.tsx` | 修改 |

---

## 数据层变更

### Store - `push-review-store.ts`

新增 `removeItem(id)` 方法：

```typescript
removeItem: (id: string) => void
```

实现：从 `items` 数组中过滤掉指定 id 的项。

### 日志持久化 - `push-review.ts`

新增日志文件 `.llm-wiki/push-review-log.json`，结构：

```typescript
interface RemoveLogEntry {
  id: string
  path: string
  removedAt: number // timestamp
  removedBy?: string
}
```

新增函数：

```typescript
export async function appendRemoveLog(projectPath: string, entry: RemoveLogEntry): Promise<void>
```

写入逻辑：读取现有日志，追加新条目，回写。

---

## 桥接层变更 - `push-review-bridge.ts`

### Payload

```typescript
interface PushReviewRemovePayload {
  id: string
}
```

### 事件监听

新增 `push-review:remove` 事件监听 `handlePushReviewRemove`，流程：

1. 从 `usePushReviewStore.getState().items` 找到对应 item（获取 path）
2. 调用 `usePushReviewStore.getState().removeItem(id)` 从 store 删除
3. 调用 `appendRemoveLog()` 写入日志

---

## UI 层变更

### PushReviewCard - `push-review-card.tsx`

| 状态 | 新增按钮 |
|------|----------|
| pending | 现有按钮（approve/reject/edit/addNotes）不变 |
| approved / rejected | 新增「移除」「阅览」按钮，替换原有操作按钮区域 |

**移除按钮**：点击后调用 `onRemove(item.id)`

**阅览按钮**：点击后调用 `onView(item.id)`，将 item 传入 Modal

### PushReviewModal - `push-review-modal.tsx`

Props 新增可选字段：

```typescript
interface PushReviewModalProps {
  // ... existing
  readOnly?: boolean // default false
  isViewMode?: boolean  // when true, show content tab for viewing
}
```

**行为变更**：
- `readOnly: true` 时隐藏 DialogFooter 的保存按钮，仅显示关闭按钮
- 阅览模式时标签显示为"内容"而非"Content"

---

## 数据流

```
用户点击「移除」
  → ReviewCard.onRemove(id)
  → push-review-bridge.ts 监听 "push-review:remove"
  → handlePushReviewRemove(id)
    → store.removeItem(id)          // 从 UI 列表和 saveQueue 中清除
    → appendRemoveLog(projectPath, entry)  // 写入 push-review-log.json

用户点击「阅览」
  → ReviewCard.onView(id)
  → ReviewModal 弹窗（readOnly=true）
  → 用户查看内容 → 关闭
```

---

## 日志格式

`.llm-wiki/push-review-log.json` 结构：

```json
{
  "version": 1,
  "entries": [
    {
      "id": "push-1234567890-1",
      "path": "my-docs/report.md",
      "removedAt": 1750000000000
    }
  ]
}
```

---

## 错误处理

- 日志写入失败：仅 console.error，不阻断移除流程
- 移除时 item 不存在：noop，不报错
- 阅览时 item 不存在：关闭 Modal

---

## 测试要点

1. pending状态仍有原有按钮，不显示移除/阅览
2. approved/rejected 状态显示移除/阅览
3. 移除后列表更新，刷新后已移除项不出现
4. 阅览 Modal 为只读，无保存按钮
5. 日志文件正确追加