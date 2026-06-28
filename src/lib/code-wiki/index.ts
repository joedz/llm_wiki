export {
  RAW_CODE_ROOT,
  WIKI_CODE_ROOT,
  CODEGRAPH_DIR,
  type AnalysisMeta,
  type CodeGraph,
  type CodeReference,
  type CodeRelationship,
  type CodeSnippet,
  type CodeWikiIndex,
  type Complexity,
  type EdgeDirection,
  type EdgeType,
  type GraphEdge,
  type GraphNode,
  type KnowledgeGraph,
  type Layer,
  type NodeType,
  type ProjectMeta,
  type RepoSummary,
  type TourStep,
} from "./types"

export {
  readKnowledgeGraph,
  writeKnowledgeGraph,
  readIndex,
  writeIndex,
  readMeta,
  writeMeta,
  knowledgeGraphPathFor,
  repoRootFor,
  metaPathFor,
  indexPathFor,
  codegraphDirFor,
} from "./wiki-storage"
export { detectRepos } from "./repo-detector"
export { buildIndex } from "./index-builder"
export { queryGraph, type GraphQueryInput, type GraphQueryResult } from "./graph-query"
export { buildKnowledgeGraph, type WriteKnowledgeGraphInput } from "./knowledge-graph-writer"
export { buildGraphForRepo, syncGraphForRepo } from "./graph-builder"
