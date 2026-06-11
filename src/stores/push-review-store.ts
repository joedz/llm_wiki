import { create } from "zustand"

export interface PushQueueItem {
  id: string
  path: string
  content: string
  contentType: "text"
  fileSize?: number
  submittedAt: number
  submittedBy?: string
  status: "pending" | "approved" | "rejected"
  notes?: string
  reviewNotes?: string
  reviewedAt?: number
  reviewedBy?: string
}

interface PushReviewState {
  items: PushQueueItem[]
  addItem: (item: Omit<PushQueueItem, "id" | "status" | "submittedAt">) => { id: string }
  approveItem: (id: string, reviewedBy?: string) => void
  rejectItem: (id: string, reviewedBy?: string) => void
  updateItem: (id: string, patch: Partial<Pick<PushQueueItem, "reviewNotes">>) => void
  resetStore: () => void
  setItems: (items: PushQueueItem[]) => void
  removeItem: (id: string) => void
}

let counter = 0

export const usePushReviewStore = create<PushReviewState>((set) => ({
  items: [],

  addItem: (item) => {
    let newId = ""
    set((state) => {
      newId = `push-${Date.now()}-${++counter}`
      return {
        items: [
          ...state.items,
          {
            ...item,
            id: newId,
            status: "pending",
            submittedAt: Date.now(),
          },
        ],
      }
    })
    return { id: newId }
  },

  approveItem: (id, reviewedBy) =>
    set((state) => ({
      items: state.items.map((item) =>
        item.id === id
          ? { ...item, status: "approved", reviewedAt: Date.now(), reviewedBy }
          : item
      ),
    })),

  rejectItem: (id, reviewedBy) =>
    set((state) => ({
      items: state.items.map((item) =>
        item.id === id
          ? { ...item, status: "rejected", reviewedAt: Date.now(), reviewedBy }
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

  removeItem: (id) =>
    set((state) => ({
      items: state.items.filter((item) => item.id !== id),
    })),
}))

