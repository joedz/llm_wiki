import { useCallback, useState } from "react"
import { CheckCircle2 } from "lucide-react"
import { useTranslation } from "react-i18next"
import { usePushReviewStore, type PushQueueItem } from "@/stores/push-review-store"
import { PushReviewCard } from "./push-review-card"
import { PushReviewModal } from "./push-review-modal"

export function PushReviewView() {
  const { t } = useTranslation()
  const items = usePushReviewStore((s) => s.items)
  const approveItem = usePushReviewStore((s) => s.approveItem)
  const rejectItem = usePushReviewStore((s) => s.rejectItem)
  const updateItem = usePushReviewStore((s) => s.updateItem)

  const [editingItem, setEditingItem] = useState<PushQueueItem | null>(null)
  const [isModalOpen, setIsModalOpen] = useState(false)

  const pending = items.filter((i) => i.status === "pending")

  const handleApprove = useCallback((id: string) => {
    approveItem(id)
  }, [approveItem])

  const handleReject = useCallback((id: string) => {
    rejectItem(id)
  }, [rejectItem])

  const handleEdit = useCallback((id: string) => {
    const item = items.find((i) => i.id === id)
    if (item) {
      setEditingItem(item)
      setIsModalOpen(true)
    }
  }, [items])

  const handleAddNotes = useCallback((id: string, notes: string) => {
    updateItem(id, { reviewNotes: notes })
  }, [updateItem])

  const handleModalSave = useCallback((id: string, reviewNotes: string) => {
    updateItem(id, { reviewNotes })
    setIsModalOpen(false)
    setEditingItem(null)
  }, [updateItem])

  const handleModalCancel = useCallback(() => {
    setIsModalOpen(false)
    setEditingItem(null)
  }, [])

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b px-4 py-3">
        <h2 className="text-sm font-semibold">
          {t("pushReview.title")}
          {pending.length > 0 && (
            <span className="ml-2 rounded-full bg-primary px-2 py-0.5 text-xs text-primary-foreground">
              {pending.length}
            </span>
          )}
        </h2>
      </div>

      <div className="flex-1 overflow-y-auto">
        {items.length === 0 ? (
          <div className="flex flex-col items-center justify-center gap-2 p-8 text-center text-sm text-muted-foreground">
            <CheckCircle2 className="h-8 w-8 text-muted-foreground/30" />
            <p>{t("pushReview.empty")}</p>
          </div>
        ) : (
          <div className="flex flex-col gap-2 p-3">
            {items.map((item) => (
              <PushReviewCard
                key={item.id}
                item={item}
                onApprove={handleApprove}
                onReject={handleReject}
                onEdit={handleEdit}
                onAddNotes={handleAddNotes}
              />
            ))}
          </div>
        )}
      </div>

      <PushReviewModal
        item={editingItem}
        isOpen={isModalOpen}
        onSave={handleModalSave}
        onCancel={handleModalCancel}
      />
    </div>
  )
}