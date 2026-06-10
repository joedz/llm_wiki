import { listen, type UnlistenFn } from "@tauri-apps/api/event"
import { approveAndIngest, loadQueue } from "@/lib/push-review"
import { useWikiStore } from "@/stores/wiki-store"
import { usePushReviewStore, type PushQueueItem } from "@/stores/push-review-store"

interface PushReviewSubmitPayload {
  id: string
  path: string
  content: string
  notes: string
  submittedBy: string
  status: string
  reviewNotes: string
}

interface PushReviewApprovePayload {
  id: string
  path: string
  content: string
}

interface PushReviewRejectPayload {
  id: string
}

interface PushReviewUpdatePayload {
  id: string
  content?: string
  reviewNotes?: string
}

function mapRustPushItemToQueueItem(payload: PushReviewSubmitPayload): Omit<PushQueueItem, "id" | "status" | "submittedAt"> {
  return {
    path: payload.path,
    content: payload.content,
    contentType: "text",
    submittedBy: payload.submittedBy,
    notes: payload.notes,
    reviewNotes: payload.reviewNotes,
  }
}

let bridgePromise: Promise<UnlistenFn> | null = null

async function handlePushReviewSubmit(payload: PushReviewSubmitPayload): Promise<void> {
  const itemData = mapRustPushItemToQueueItem(payload)
  usePushReviewStore.getState().addItem(itemData)
}

async function handlePushReviewGetQueue(): Promise<void> {
  const project = useWikiStore.getState().project
  if (!project) return
  const items = await loadQueue(project.path, project.id)
  usePushReviewStore.getState().setItems(items)
}

async function handlePushReviewApprove(payload: PushReviewApprovePayload): Promise<void> {
  const items = usePushReviewStore.getState().items
  const item = items.find((i) => i.id === payload.id)
  if (item) {
    await approveAndIngest(item)
  }
}

async function handlePushReviewReject(payload: PushReviewRejectPayload): Promise<void> {
  usePushReviewStore.getState().rejectItem(payload.id)
}

async function handlePushReviewUpdate(payload: PushReviewUpdatePayload): Promise<void> {
  usePushReviewStore.getState().updateItem(payload.id, { reviewNotes: payload.reviewNotes })
}

export function ensurePushReviewBridge(): Promise<UnlistenFn> {
  if (!bridgePromise) {
    bridgePromise = (async () => {
      const unlistenSubmit = await listen<PushReviewSubmitPayload>("push-review:submit", (event) => {
        void handlePushReviewSubmit(event.payload)
      })
      const unlistenGetQueue = await listen("push-review:get-queue", () => {
        void handlePushReviewGetQueue()
      })
      const unlistenApprove = await listen<PushReviewApprovePayload>("push-review:approve", (event) => {
        void handlePushReviewApprove(event.payload)
      })
      const unlistenReject = await listen<PushReviewRejectPayload>("push-review:reject", (event) => {
        void handlePushReviewReject(event.payload)
      })
      const unlistenUpdate = await listen<PushReviewUpdatePayload>("push-review:update", (event) => {
        void handlePushReviewUpdate(event.payload)
      })
      return () => {
        unlistenSubmit()
        unlistenGetQueue()
        unlistenApprove()
        unlistenReject()
        unlistenUpdate()
      }
    })()
  }
  return bridgePromise
}