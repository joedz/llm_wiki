import { fileExists, readFile, writeFile, createDirectory } from "@/commands/fs"
import {
  CODEGRAPH_DIR,
  WIKI_CODE_ROOT,
  type CodeGraph,
  type CodeWikiIndex,
  type CodeWikiMeta,
} from "./types"

const CODEGRAPH_ROOT_DIR = CODEGRAPH_DIR

export function repoRootFor(projectPath: string, repoName: string): string {
  return `${projectPath}/${WIKI_CODE_ROOT}/${repoName}`
}

export function graphPathFor(projectPath: string, repoName: string): string {
  return `${repoRootFor(projectPath, repoName)}/graph.json`
}

export function metaPathFor(projectPath: string, repoName: string): string {
  return `${repoRootFor(projectPath, repoName)}/meta.json`
}

export function indexPathFor(projectPath: string): string {
  return `${projectPath}/${WIKI_CODE_ROOT}/index.json`
}

export function codegraphDirFor(projectPath: string, repoName: string): string {
  return `${repoRootFor(projectPath, repoName)}/${CODEGRAPH_ROOT_DIR}`
}

async function ensureParent(path: string): Promise<void> {
  const slash = path.lastIndexOf("/")
  if (slash < 0) return
  const parent = path.slice(0, slash)
  await createDirectory(parent)
}

export async function writeGraph(projectPath: string, repoName: string, graph: CodeGraph): Promise<void> {
  const path = graphPathFor(projectPath, repoName)
  await ensureParent(path)
  await writeFile(path, JSON.stringify(graph, null, 2))
}

export async function readGraph(projectPath: string, repoName: string): Promise<CodeGraph | null> {
  const path = graphPathFor(projectPath, repoName)
  if (!(await fileExists(path))) return null
  const raw = await readFile(path)
  return JSON.parse(raw) as CodeGraph
}

export async function writeMeta(projectPath: string, repoName: string, meta: CodeWikiMeta): Promise<void> {
  const path = metaPathFor(projectPath, repoName)
  await ensureParent(path)
  await writeFile(path, JSON.stringify(meta, null, 2))
}

export async function readMeta(projectPath: string, repoName: string): Promise<CodeWikiMeta | null> {
  const path = metaPathFor(projectPath, repoName)
  if (!(await fileExists(path))) return null
  const raw = await readFile(path)
  return JSON.parse(raw) as CodeWikiMeta
}

export async function writeIndex(projectPath: string, index: CodeWikiIndex): Promise<void> {
  const path = indexPathFor(projectPath)
  await ensureParent(path)
  await writeFile(path, JSON.stringify(index, null, 2))
}

export async function readIndex(projectPath: string): Promise<CodeWikiIndex> {
  const path = indexPathFor(projectPath)
  if (!(await fileExists(path))) return { version: "1.0.0", generatedAt: "", repos: [] }
  const raw = await readFile(path)
  return JSON.parse(raw) as CodeWikiIndex
}