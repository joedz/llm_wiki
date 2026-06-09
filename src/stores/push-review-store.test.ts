import { describe, it, expect, beforeEach } from "vitest"
import { usePushReviewStore } from "./push-review-store"

describe("push-review-store", () => {
  beforeEach(() => {
    usePushReviewStore.getState().resetStore()
  })

  it("starts with empty queue", () => {
    expect(usePushReviewStore.getState().items).toEqual([])
  })

  it("adds an item", () => {
    const { id } = usePushReviewStore.getState().addItem({
      path: "my-docs/test.md",
      content: "# Test",
      contentType: "text",
      submittedBy: "MCP",
    })
    expect(usePushReviewStore.getState().items).toHaveLength(1)
    expect(usePushReviewStore.getState().items[0].status).toBe("pending")
    expect(id).toMatch(/^push-\d+-\d+$/)
  })

  it("approves an item", () => {
    const { id } = usePushReviewStore.getState().addItem({
      path: "test.md",
      content: "# Test",
      contentType: "text",
      submittedBy: "MCP",
    })
    usePushReviewStore.getState().approveItem(id, "reviewer-1")
    const item = usePushReviewStore.getState().items.find((i) => i.id === id)
    expect(item?.status).toBe("approved")
    expect(item?.reviewedBy).toBe("reviewer-1")
    expect(item?.reviewedAt).toBeDefined()
  })

  it("rejects an item", () => {
    const { id } = usePushReviewStore.getState().addItem({
      path: "test.md",
      content: "# Test",
      contentType: "text",
      submittedBy: "MCP",
    })
    usePushReviewStore.getState().rejectItem(id, "reviewer-1")
    const item = usePushReviewStore.getState().items.find((i) => i.id === id)
    expect(item?.status).toBe("rejected")
    expect(item?.reviewedBy).toBe("reviewer-1")
    expect(item?.reviewedAt).toBeDefined()
  })

  it("updates review notes", () => {
    const { id } = usePushReviewStore.getState().addItem({
      path: "test.md",
      content: "# Test",
      contentType: "text",
      submittedBy: "MCP",
    })
    usePushReviewStore.getState().updateItem(id, { reviewNotes: "Looks good" })
    const item = usePushReviewStore.getState().items.find((i) => i.id === id)
    expect(item?.reviewNotes).toBe("Looks good")
  })

  it("sets items directly", () => {
    const items = [
      {
        id: "push-123-1",
        path: "test.md",
        content: "# Test",
        contentType: "text" as const,
        status: "pending" as const,
        submittedAt: Date.now(),
      },
    ]
    usePushReviewStore.getState().setItems(items)
    expect(usePushReviewStore.getState().items).toHaveLength(1)
  })
})