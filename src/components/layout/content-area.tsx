import { useWikiStore } from "@/stores/wiki-store"
import { ChatPanel } from "@/components/chat/chat-panel"
import { SettingsView } from "@/components/settings/settings-view"
import { SourcesView } from "@/components/sources/sources-view"
import { ReviewView } from "@/components/review/review-view"
import { LintView } from "@/components/lint/lint-view"
import { SearchView } from "@/components/search/search-view"
import { GraphView } from "@/components/graph/graph-view"
import { PushReviewView } from "@/components/push-review/push-review-view"
import { CodeWikiView } from "@/components/code-wiki/code-wiki-view"

export function ContentArea() {
  const activeView = useWikiStore((s) => s.activeView)

  switch (activeView) {
    case "settings":
      return <SettingsView />
    case "sources":
      return <SourcesView />
    case "review":
      return <ReviewView />
    case "pushReview":
      return <PushReviewView />
    case "lint":
      return <LintView />
    case "search":
      return <SearchView />
    case "graph":
      return <GraphView />
    case "codeWiki":
      return <CodeWikiView />
    default:
      return <ChatPanel />
  }
}
