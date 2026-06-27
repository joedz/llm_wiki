import { readFile } from "@/commands/fs"
import { anyTxtSearchSmart } from "@/lib/anytxt-search"
import { computeContextBudget } from "@/lib/context-budget"
import { buildRetrievalGraph, getRelatedNodes } from "@/lib/graph-relevance"
import { getFileName, getRelativePath, normalizePath } from "@/lib/path-utils"
import { searchWiki, tokenizeQuery, type SearchResult } from "@/lib/search"
import { resolveSearchConfig, webSearch, type WebSearchResult } from "@/lib/web-search"
import { buildCodeAnalysisContext, type CodeAnalysisContext } from "./code-analysis"
import { readIndex as readCodeWikiIndex, readGraph as readCodeWikiGraph, queryGraph } from "@/lib/code-wiki"
import type { ChatRuntimeConfig } from "./chat-runtime-config"

export interface ChatReference {
  title: string
  path: string
  kind: "wiki" | "external" | "code"
  source?: string
  url?: string
  snippet?: string
}

export interface ChatRetrievedPage {
  id: string
  title: string
  path: string
  content: string
  priority: number
}

export interface ChatRetrievedContext {
  purpose: string
  index: string
  wikiPages: ChatRetrievedPage[]
  codeContext: CodeAnalysisContext | null
  externalResults: WebSearchResult[]
  references: ChatReference[]
  warnings: string[]
}

export interface BuildChatRetrievalContextInput {
  projectPath: string
  projectName: string
  message: string
  useWebSearch: boolean
  useAnyTxtSearch: boolean
  config: ChatRuntimeConfig
}

const TRUNCATION_SUFFIX = "\n\n[...truncated...]"

function buildPageId(filePath: string, projectPath: string): string {
  return getRelativePath(filePath, projectPath)
    .replace(/\.md$/i, "")
    .replace(/[^a-zA-Z0-9/_-]+/g, "-")
}

function fitPageContent(raw: string, limit: number): string {
  if (limit <= 0) return ""
  if (raw.length <= limit) return raw
  if (limit <= TRUNCATION_SUFFIX.length) return raw.slice(0, limit)

  return `${raw.slice(0, limit - TRUNCATION_SUFFIX.length)}${TRUNCATION_SUFFIX}`
}

function resolveGraphNodeId(
  result: SearchResult,
  graph: Awaited<ReturnType<typeof buildRetrievalGraph>>,
): string {
  const exactPathMatch = [...graph.nodes.values()].find((node) => node.path === result.path)
  if (exactPathMatch) return exactPathMatch.id

  return getFileName(result.path).replace(/\.md$/i, "")
}

function dedupeExternalResults(results: WebSearchResult[]): WebSearchResult[] {
  const seen = new Set<string>()
  const deduped: WebSearchResult[] = []

  for (const result of results) {
    const key = result.url || `${result.source}:${result.title}:${result.snippet}`
    if (seen.has(key)) continue
    seen.add(key)
    deduped.push(result)
  }

  return deduped
}

function trimIndex(rawIndex: string, message: string, indexBudget: number): string {
  if (rawIndex.length <= indexBudget) return rawIndex

  const tokens = tokenizeQuery(message)
  const lines = rawIndex.split("\n")
  const keptLines: string[] = []
  let keptSize = 0

  for (const line of lines) {
    const isHeader = line.startsWith("##")
    const lower = line.toLowerCase()
    const isRelevant = tokens.some((token) => lower.includes(token))

    if (!isHeader && !isRelevant) continue
    if (keptSize + line.length + 1 > indexBudget) break

    keptLines.push(line)
    keptSize += line.length + 1
  }

  const trimmed = keptLines.join("\n")
  if (!trimmed) return rawIndex.slice(0, indexBudget)
  if (trimmed.length === rawIndex.length) return trimmed
  return `${trimmed}\n\n[...index trimmed to relevant entries...]`
}

async function collectExternalResults(
  input: BuildChatRetrievalContextInput,
  projectPath: string,
): Promise<{ externalResults: WebSearchResult[]; warnings: string[] }> {
  const resolvedSearchConfig = resolveSearchConfig(input.config.searchApiConfig)
  const warnings: string[] = []
  const calls: Promise<WebSearchResult[]>[] = []

  if (input.useWebSearch) {
    calls.push(
      webSearch(input.message, resolvedSearchConfig, 5).catch((error) => {
        warnings.push(`Web Search: ${error instanceof Error ? error.message : String(error)}`)
        return []
      }),
    )
  }

  if (input.useAnyTxtSearch) {
    calls.push(
      anyTxtSearchSmart(
        input.message,
        resolvedSearchConfig.anyTxt,
        input.config.llmConfig,
        5,
        projectPath,
      ).catch((error) => {
        warnings.push(`AnyTXT: ${error instanceof Error ? error.message : String(error)}`)
        return []
      }),
    )
  }

  if (calls.length === 0) {
    return { externalResults: [], warnings }
  }

  const batches = await Promise.all(calls)
  return {
    externalResults: dedupeExternalResults(batches.flat()).slice(0, 10),
    warnings,
  }
}

async function buildRelevantPages(
  projectPath: string,
  message: string,
  maxContextSize: number | undefined,
  dataVersion: number,
): Promise<ChatRetrievedPage[]> {
  const { pageBudget, maxPageSize } = computeContextBudget(maxContextSize)
  const searchResults = await searchWiki(projectPath, message)
  const topSearchResults = searchResults.slice(0, 10)

  const graph = await buildRetrievalGraph(projectPath, dataVersion)
  const searchHitPaths = new Set(topSearchResults.map((result) => result.path))
  const expandedIds = new Set<string>()
  const graphExpansions: Array<{ title: string; path: string; relevance: number }> = []

  for (const result of topSearchResults) {
    const nodeId = resolveGraphNodeId(result, graph)
    const related = getRelatedNodes(nodeId, graph, 3)
    for (const { node, relevance } of related) {
      if (relevance < 2.0) continue
      if (searchHitPaths.has(node.path)) continue
      if (expandedIds.has(node.id)) continue
      expandedIds.add(node.id)
      graphExpansions.push({ title: node.title, path: node.path, relevance })
    }
  }

  graphExpansions.sort((a, b) => b.relevance - a.relevance)

  let usedChars = 0
  const relevantPages: ChatRetrievedPage[] = []

  const tryAddPage = async (title: string, filePath: string, priority: number): Promise<boolean> => {
    if (usedChars >= pageBudget) return false

    try {
      const raw = await readFile(filePath)
      const remainingBudget = pageBudget - usedChars
      const contentLimit = Math.min(maxPageSize, remainingBudget)
      const content = fitPageContent(raw, contentLimit)

      if (!content) return false

      usedChars += content.length
      relevantPages.push({
        id: buildPageId(filePath, projectPath),
        title,
        path: getRelativePath(filePath, projectPath),
        content,
        priority,
      })
      return true
    } catch {
      return false
    }
  }

  const addPagesFor = async (
    results: SearchResult[],
    priority: number,
  ): Promise<void> => {
    for (const result of results) {
      await tryAddPage(result.title, result.path, priority)
    }
  }

  await addPagesFor(topSearchResults.filter((result) => result.titleMatch), 0)
  await addPagesFor(topSearchResults.filter((result) => !result.titleMatch), 1)

  for (const expansion of graphExpansions) {
    await tryAddPage(expansion.title, expansion.path, 2)
  }

  if (relevantPages.length === 0) {
    await tryAddPage("Overview", `${projectPath}/wiki/overview.md`, 3)
  }

  return relevantPages
}

async function buildCodeWikiOrFallbackContext(
  projectPath: string,
  message: string,
  maxContextSize: number,
): Promise<CodeAnalysisContext | null> {
  try {
    const index = await readCodeWikiIndex(projectPath)
    if (index.repos.length > 0) {
      const snippets: CodeAnalysisContext["snippets"] = []
      const relationships: CodeAnalysisContext["relationships"] = []
      const references: CodeAnalysisContext["references"] = []
      let totalChars = 0
      const perRepoBudget = Math.max(2_000, Math.floor(maxContextSize / index.repos.length))
      for (const repo of index.repos) {
        if (totalChars >= maxContextSize) break
        const graph = await readCodeWikiGraph(projectPath, repo.name)
        if (!graph) continue
        const result = queryGraph({ graph, message, hops: 1, maxContextSize: perRepoBudget })
        for (const s of result.snippets) {
          if (totalChars + s.content.length > maxContextSize) break
          snippets.push(s)
          totalChars += s.content.length
        }
        relationships.push(...result.relationships)
        references.push(...result.references)
        if (snippets.length === 0 && relationships.length === 0) continue
      }
      if (snippets.length > 0 || relationships.length > 0) {
        return { snippets, relationships, references }
      }
    }
  } catch (err) {
    console.warn("[code-wiki] query failed, falling back to live scan:", err)
  }
  return buildCodeAnalysisContext({ projectPath, message, maxContextSize })
}

export async function buildChatRetrievalContext(
  input: BuildChatRetrievalContextInput,
): Promise<ChatRetrievedContext> {
  const projectPath = normalizePath(input.projectPath)
  const { indexBudget } = computeContextBudget(input.config.llmConfig.maxContextSize)

  const [rawIndex, purpose, wikiPages, codeContext, external] = await Promise.all([
    readFile(`${projectPath}/wiki/index.md`).catch(() => ""),
    readFile(`${projectPath}/purpose.md`).catch(() => ""),
    buildRelevantPages(
      projectPath,
      input.message,
      input.config.llmConfig.maxContextSize,
      input.config.dataVersion,
    ),
    buildCodeWikiOrFallbackContext(projectPath, input.message, input.config.llmConfig.maxContextSize),
    collectExternalResults(input, projectPath),
  ])

  const references: ChatReference[] = [
    ...wikiPages.map((page) => ({
      title: page.title,
      path: page.path,
      kind: "wiki" as const,
      snippet: page.content.slice(0, 240),
    })),
    ...external.externalResults.map((result) => ({
      title: result.title,
      path: result.url,
      kind: "external" as const,
      source: result.source,
      url: result.url,
      snippet: result.snippet,
    })),
    ...(codeContext?.references ?? []),
  ]

  return {
    purpose,
    index: trimIndex(rawIndex, input.message, indexBudget),
    wikiPages,
    codeContext,
    externalResults: external.externalResults,
    references,
    warnings: external.warnings,
  }
}
