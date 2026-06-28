import { readKnowledgeGraph, writeIndex } from "./wiki-storage"
import { RAW_CODE_ROOT, WIKI_CODE_ROOT, type CodeWikiIndex, type RepoSummary } from "./types"

export async function buildIndex(projectPath: string, repoNames: string[]): Promise<CodeWikiIndex> {
  const repos: RepoSummary[] = []
  for (const name of repoNames) {
    const graph = await readKnowledgeGraph(projectPath, name)
    if (!graph) continue
    const fileCount = graph.nodes.filter((n) => n.type === "file").length
    const symbolCount = graph.nodes.length - fileCount
    repos.push({
      name,
      path: `${RAW_CODE_ROOT}/${name}`,
      graphPath: `${WIKI_CODE_ROOT}/${name}/knowledge-graph.json`,
      languages: graph.project.languages,
      fileCount,
      symbolCount,
      description: graph.project.description,
      lastAnalyzedAt: graph.project.analyzedAt,
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
