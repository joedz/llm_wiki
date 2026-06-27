import { listDirectory } from "@/commands/fs"
import { RAW_CODE_ROOT } from "./types"

export async function detectRepos(projectPath: string): Promise<string[]> {
  const codeRoot = `${projectPath}/${RAW_CODE_ROOT}`
  let entries: Awaited<ReturnType<typeof listDirectory>>
  try {
    entries = await listDirectory(codeRoot)
  } catch {
    return []
  }
  const repos: string[] = []
  for (const entry of entries) {
    if (!entry.is_dir) continue
    if (entry.name.startsWith(".")) continue
    if (entry.name === "node_modules") continue
    repos.push(entry.name)
  }
  return repos
}