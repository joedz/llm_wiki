import { useState } from "react"
import { Check, X, Pencil, MessageSquare, FileText, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import { useTranslation } from "react-i18next"
import type { PushQueueItem } from "@/stores/push-review-store"

interface PushReviewCardProps {
  item: PushQueueItem
  onApprove: (id: string) => void
  onReject: (id: string) => void
  onEdit: (id: string) => void
  onAddNotes: (id: string, notes: string) => void
  onRemove: (id: string) => void
  onView: (id: string) => void
}

const statusConfig: Record<PushQueueItem["status"], { color: string; label: string }> = {
  pending: { color: "text-amber-500", label: "Pending" },
  approved: { color: "text-emerald-500", label: "Approved" },
  rejected: { color: "text-red-500", label: "Rejected" },
}

export function PushReviewCard({ item, onApprove, onReject, onEdit, onAddNotes, onRemove, onView }: PushReviewCardProps) {
  const { t } = useTranslation()
  const [notesInput, setNotesInput] = useState("")
  const [showNotesInput, setShowNotesInput] = useState(false)
  const config = statusConfig[item.status]

  const handleSubmitNotes = () => {
    if (notesInput.trim()) {
      onAddNotes(item.id, notesInput.trim())
      setNotesInput("")
      setShowNotesInput(false)
    }
  }

  return (
    <div className={`rounded-lg border p-3 text-sm ${item.status !== "pending" ? "opacity-60" : ""}`}>
      <div className="mb-2 flex items-start justify-between gap-2">
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium">{item.path}</p>
          <div className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
            <span>{item.contentType}</span>
            {item.fileSize &&<span>{(item.fileSize / 1024).toFixed(1)} KB</span>}
            <span>{new Date(item.submittedAt).toLocaleString()}</span>
          </div>
        </div>
        <span className={`shrink-0 text-xs font-medium ${config.color}`}>
          {config.label}
        </span>
      </div>

      {item.submittedBy && (
        <div className="mb-2">
          <span className="inline-flex rounded bg-muted px-1.5 py-0.5 text-xs">
            {item.submittedBy}
          </span>
        </div>
      )}

      {item.notes && (
        <p className="mb-2 text-xs text-muted-foreground">{item.notes}</p>
      )}

      {item.reviewNotes && (
        <div className="mb-2 rounded bg-muted/50 p-2 text-xs">
          <span className="font-medium">Review Notes: </span>
          <span className="text-muted-foreground">{item.reviewNotes}</span>
        </div>
      )}

      {showNotesInput && (
        <div className="mb-2 flex flex-col gap-1.5">
          <textarea
            className="w-full rounded-md border border-input bg-background px-2 py-1 text-xs resize-none"
            rows={2}
            value={notesInput}
            onChange={(e) => setNotesInput(e.target.value)}
            placeholder={t("pushReview.addNotesPlaceholder")}
          />
          <div className="flex gap-1.5">
            <Button size="sm" className="h-6 text-xs" onClick={handleSubmitNotes}>
              {t("pushReview.save")}
            </Button>
            <Button size="sm" variant="ghost" className="h-6 text-xs" onClick={() => setShowNotesInput(false)}>
              {t("pushReview.cancel")}
            </Button>
          </div>
        </div>
      )}

      {item.status === "pending" && (
        <div className="flex flex-wrap gap-1.5">
          <Button
            variant="outline"
            size="sm"
            className="h-7 text-xs gap-1"
            onClick={() => onApprove(item.id)}
          >
            <Check className="h-3 w-3" />
            {t("pushReview.approve")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="h-7 text-xs gap-1"
            onClick={() => onReject(item.id)}
          >
            <X className="h-3 w-3" />
            {t("pushReview.reject")}
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="h-7 text-xs gap-1"
            onClick={() => onEdit(item.id)}
          >
            <Pencil className="h-3 w-3" />
            {t("pushReview.edit")}
          </Button>
          {!showNotesInput && (
            <Button
              variant="ghost"
              size="sm"
              className="h-7 text-xs gap-1"
              onClick={() => setShowNotesInput(true)}
            >
              <MessageSquare className="h-3 w-3" />
              {t("pushReview.addNotes")}
            </Button>
          )}
        </div>
      )}
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
    </div>
  )
}