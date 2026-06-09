import { useState } from "react"
import { useTranslation } from "react-i18next"
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import type { PushQueueItem } from "@/stores/push-review-store"

interface PushReviewModalProps {
  item: PushQueueItem | null
  onSave: (id: string, reviewNotes: string) => void
  onCancel: () => void
  isOpen: boolean
}

export function PushReviewModal({ item, onSave, onCancel, isOpen }: PushReviewModalProps) {
  const { t } = useTranslation()
  const [reviewNotes, setReviewNotes] = useState(item?.reviewNotes ?? "")

  if (item) {
    return (
      <Dialog open={isOpen} onOpenChange={(open) => !open && onCancel()}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>{item.path}</DialogTitle>
          </DialogHeader>

          <div className="flex flex-col gap-3">
            <div>
              <label className="text-xs font-medium text-muted-foreground mb-1 block">
                Content
              </label>
              <textarea
                className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-xs resize-none"
                rows={12}
                value={item.content}
                readOnly
              />
            </div>

            <div>
              <label className="text-xs font-medium text-muted-foreground mb-1 block">
                Review Notes
              </label>
              <textarea
                className="w-full rounded-md border border-input bg-background px-3 py-2 text-xs resize-none"
                rows={3}
                value={reviewNotes}
                onChange={(e) => setReviewNotes(e.target.value)}
                placeholder={t("pushReview.reviewNotesPlaceholder")}
              />
            </div>
          </div>

          <DialogFooter>
            <Button variant="outline" onClick={onCancel}>
              {t("pushReview.cancel")}
            </Button>
            <Button
              onClick={() => {
                if (item) {
                  onSave(item.id, reviewNotes)
                }
              }}
            >
              {t("pushReview.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    )
  }

  return null
}