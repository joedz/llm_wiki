import { readFile, writeFile, fileExists, writePushSource } from "@/commands/fs"
import { normalizePath } from "@/lib/path-utils"
import { enqueueSourceIngest } from "@/lib/source-lifecycle"
import { useWikiStore } from "@/stores/wiki-store"
import { usePushReviewStore, type PushQueueItem } from "@/stores/push-review-store"

function queueFilePath(projectPath: string): string {
  return `${normalizePath(projectPath)}/.llm-wiki/push-queue.json`
}

export async function saveQueue(
  projectPath: string,
  items: PushQueueItem[],
): Promise<void> {
  try {
    await writeFile(
      queueFilePath(projectPath),
      JSON.stringify({ version: 1, items }, null, 2),
    )
  } catch {
    // non-critical
  }
}

export async function loadQueue(
  projectPath: string,
  _projectId: string,
): Promise<PushQueueItem[]> {
  try {
    const raw = await readFile(queueFilePath(projectPath))
    const data = JSON.parse(raw) as { version: number; items: PushQueueItem[] }
    return data.items
  } catch {
    return []
  }
}

let currentProjectPath = ""

export function initPushReviewPersistence(): void {
  useWikiStore.subscribe((state) => {
    currentProjectPath = state.project?.path ?? ""
  })

  usePushReviewStore.subscribe((state) => {
    if (currentProjectPath) {
      saveQueue(currentProjectPath, state.items)
    }
  })
}

export async function restorePushReviewQueue(
  projectId: string,
  projectPath: string,
): Promise<void> {
  const pp = normalizePath(projectPath)
  const items = await loadQueue(pp, projectId)
  if (items.length > 0) {
    usePushReviewStore.getState().setItems(items)
  }
}

export function generateId(): string {
  return `push-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

async function resolveUniquePath(sourcesRoot: string, path: string): Promise<string> {
  const fullPath = `${normalizePath(sourcesRoot)}/${normalizePath(path)}`
  if (!(await fileExists(fullPath))) {
    return path
  }
  const lastDot = path.lastIndexOf(".")
  const ext = lastDot > 0 ? path.slice(lastDot) : ""
  const base = ext ? path.slice(0, -ext.length) : path
  return `${base}_1${ext}`
}

export async function approveAndIngest(item: PushQueueItem): Promise<void> {
  const project = useWikiStore.getState().project
  const llmConfig = useWikiStore.getState().llmConfig
  if (!project) throw new Error("No project open")

  const sourcesRoot = `${normalizePath(project.path)}/raw/sources`
  const relativePath = await resolveUniquePath(sourcesRoot, item.path)

  await writePushSource(project.path, relativePath, item.content)

  await enqueueSourceIngest(project, [relativePath], llmConfig)

  usePushReviewStore.getState().approveItem(item.id)
}

