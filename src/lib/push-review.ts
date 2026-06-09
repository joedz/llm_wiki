import { readFile, writeFile } from "@/commands/fs"
import { normalizePath } from "@/lib/path-utils"
import type { PushQueueItem } from "@/stores/push-review-store"

function queueFilePath(projectPath: string): string {
  return `${normalizePath(projectPath)}/.llm-wiki/push-queue.json`
}

async function saveQueue(
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

async function loadQueue(
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

function generateId(): string {
  return `push-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

export { loadQueue, saveQueue, generateId }