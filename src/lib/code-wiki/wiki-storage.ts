import { fileExists, readFile, writeFile, createDirectory } from "@/commands/fs"
import {
  CODEGRAPH_DIR,
  RAW_CODE_ROOT,
  WIKI_CODE_ROOT,
  type AnalysisMeta,
  type CodeWikiIndex,
  type KnowledgeGraph,
} from "./types"

const KNOWLEDGE_GRAPH_FILE = "knowledge-graph.json"

export function repoRootFor(projectPath: string, repoName: string): string {
  return `${projectPath}/${WIKI_CODE_ROOT}/${repoName}`
}

export function knowledgeGraphPathFor(projectPath: string, repoName: string): string {
  return `${repoRootFor(projectPath, repoName)}/${KNOWLEDGE_GRAPH_FILE}`
}

export function metaPathFor(projectPath: string, repoName: string): string {
  return `${repoRootFor(projectPath, repoName)}/meta.json`
}

export function indexPathFor(projectPath: string): string {
  return `${projectPath}/${WIKI_CODE_ROOT}/index.json`
}

/// Where codegraph 0.9.x writes its SQLite DB for a given repo. codegraph
/// always uses `<repo_path>/.codegraph/` (no flag to override), so for an
/// imported repo under `raw/code/<repo>/` the DB lives there too. Hidden
/// directory, so it doesn't pollute the user's source tree in practice.
export function codegraphDirFor(projectPath: string, repoName: string): string {
  return `${projectPath}/${RAW_CODE_ROOT}/${repoName}/${CODEGRAPH_DIR}`
}

async function ensureParent(path: string): Promise<void> {
  const slash = path.lastIndexOf("/")
  if (slash < 0) return
  const parent = path.slice(0, slash)
  await createDirectory(parent)
}

/** Write the canonical knowledge-graph.json (UA shape). */
export async function writeKnowledgeGraph(
  projectPath: string,
  repoName: string,
  graph: KnowledgeGraph,
): Promise<void> {
  const path = knowledgeGraphPathFor(projectPath, repoName)
  await ensureParent(path)
  await writeFile(path, JSON.stringify(graph, null, 2))
}

/** Read the canonical knowledge-graph.json. Returns null if missing. */
export async function readKnowledgeGraph(
  projectPath: string,
  repoName: string,
): Promise<KnowledgeGraph | null> {
  const path = knowledgeGraphPathFor(projectPath, repoName)
  if (!(await fileExists(path))) return null
  const raw = await readFile(path)
  return JSON.parse(raw) as KnowledgeGraph
}

export async function writeMeta(
  projectPath: string,
  repoName: string,
  meta: AnalysisMeta,
): Promise<void> {
  const path = metaPathFor(projectPath, repoName)
  await ensureParent(path)
  await writeFile(path, JSON.stringify(meta, null, 2))
}

export async function readMeta(
  projectPath: string,
  repoName: string,
): Promise<AnalysisMeta | null> {
  const path = metaPathFor(projectPath, repoName)
  if (!(await fileExists(path))) return null
  const raw = await readFile(path)
  return JSON.parse(raw) as AnalysisMeta
}

export async function writeIndex(
  projectPath: string,
  index: CodeWikiIndex,
): Promise<void> {
  const path = indexPathFor(projectPath)
  await ensureParent(path)
  await writeFile(path, JSON.stringify(index, null, 2))
}

export async function readIndex(projectPath: string): Promise<CodeWikiIndex> {
  const path = indexPathFor(projectPath)
  if (!(await fileExists(path))) {
    return { version: "1.0.0", generatedAt: "", repos: [] }
  }
  const raw = await readFile(path)
  return JSON.parse(raw) as CodeWikiIndex
}
