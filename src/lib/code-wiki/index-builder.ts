import { readGraph, writeIndex } from "./wiki-storage"
import { RAW_CODE_ROOT, WIKI_CODE_ROOT, type CodeWikiIndex, type RepoSummary } from "./types"

export async function buildIndex(projectPath: string, repoNames: string[]): Promise<CodeWikiIndex> {
  const repos: RepoSummary[] = []
  for (const name of repoNames) {
    const graph = await readGraph(projectPath, name)
    if (!graph) continue
    repos.push({
      name,
      path: `${RAW_CODE_ROOT}/${name}`,
      graphPath: `${WIKI_CODE_ROOT}/${name}/graph.json`,
      languages: graph.project.languages,
      fileCount: graph.project.fileCount,
      symbolCount: graph.project.symbolCount,
      description: graph.project.description,
      lastAnalyzedAt: graph.project.lastAnalyzedAt,
    })
  }
  repos.sort((a, b) => a.name.localeCompare(b.name))
  const index: CodeWikiIndex = {
    version: "1.0.0",
    generatedAt: new Date().toISOString(),
    repos,
  }
  await writeIndex(projectPath, index)
  return index
}