// TS client for the code-wiki explain Tauri command.

import { invoke } from "@tauri-apps/api/core"

export interface ExplainNeighbor {
  node: {
    id: string
    type: string
    name: string
    filePath?: string
    summary: string
    tags: string[]
    complexity: string
  }
  edge: {
    source: string
    target: string
    type: string
    direction: string
    weight: number
    description?: string
  }
}

export interface ExplainLayer {
  id: string
  name: string
  description: string
  nodeIds: string[]
}

export interface ExplainResult {
  nodeId: string
  markdown: string
  neighborCount: number
  sourceLinesRead: number
  usedLlm: boolean
  durationMs: number
  layer: ExplainLayer | null
}

export interface LlmRequestSpec {
  provider: "anthropic" | "openai" | "ollama" | "custom"
  apiKey: string
  model: string
  baseUrl?: string
  maxTokens?: number
  temperature?: number
}

export function explainNode(
  projectPath: string,
  repoName: string,
  nodeId: string,
  llm?: LlmRequestSpec,
): Promise<ExplainResult> {
  return invoke("code_wiki_explain_node", {
    projectPath,
    repoName,
    nodeId,
    llm,
  })
}