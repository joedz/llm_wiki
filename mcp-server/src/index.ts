#!/usr/bin/env node
import { Server } from "@modelcontextprotocol/sdk/server/index.js"
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js"
import {
  CallToolRequestSchema,
  ErrorCode,
  ListToolsRequestSchema,
  McpError,
} from "@modelcontextprotocol/sdk/types.js"
import {
  LlmWikiApiClient,
  type ApiChatReference,
  type ApiFileNode,
  type ApiGraphNode,
  type ApiSearchResult,
} from "./api-client.js"

const VERSION = "0.4.20"
const DEFAULT_PROJECT_ID = "current"
const MAX_TEXT_BYTES = 120_000

const client = new LlmWikiApiClient()

const server = new Server(
  { name: "llm-wiki", version: VERSION },
  { capabilities: { tools: {} } },
)

server.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: [
    {
      name: "llm_wiki_status",
      description: "Check whether the LLM Wiki desktop local API is reachable and list the current project.",
      inputSchema: {
        type: "object",
        properties: {},
        additionalProperties: false,
      },
    },
    {
      name: "llm_wiki_projects",
      description: "List known LLM Wiki projects. The response includes currentProject when the desktop app has an active project.",
      inputSchema: {
        type: "object",
        properties: {},
        additionalProperties: false,
      },
    },
    {
      name: "llm_wiki_files",
      description: "List files from a project using the desktop app's API permissions. project_id may be a UUID, filesystem path, or 'current'.",
      inputSchema: {
        type: "object",
        properties: {
          project_id: { type: "string", description: "Project UUID, project path, or 'current'. Defaults to current." },
          root: { type: "string", enum: ["wiki", "sources", "all"], description: "Tree root to list. Defaults to wiki." },
          recursive: { type: "boolean", description: "Whether to list recursively. Defaults to true." },
          max_files: { type: "number", description: "Maximum files returned by the local API. Max 10000." },
        },
        additionalProperties: false,
      },
    },
    {
      name: "llm_wiki_read_file",
      description: "Read a text file from a project through the desktop app API. Only public project paths such as wiki/ and raw/sources/ are allowed by the API.",
      inputSchema: {
        type: "object",
        properties: {
          project_id: { type: "string", description: "Project UUID, project path, or 'current'. Defaults to current." },
          path: { type: "string", description: "Project-relative file path, for example wiki/index.md." },
        },
        required: ["path"],
        additionalProperties: false,
      },
    },
    {
      name: "llm_wiki_search",
      description: "Search a project using the same backend keyword/vector retrieval used by the desktop API.",
      inputSchema: {
        type: "object",
        properties: {
          project_id: { type: "string", description: "Project UUID, project path, or 'current'. Defaults to current." },
          query: { type: "string", description: "Search query." },
          top_k: { type: "number", description: "Maximum results. The local API clamps to its configured maximum." },
          include_content: { type: "boolean", description: "Include full page content in results when supported by the API." },
        },
        required: ["query"],
        additionalProperties: false,
      },
    },
    {
      name: "llm_wiki_chat",
      description: "Ask the current LLM Wiki project a question through the desktop app's shared chat/RAG pipeline.",
      inputSchema: {
        type: "object",
        properties: {
          project_id: { type: "string", description: "Project UUID, project path, or 'current'. Defaults to current." },
          message: { type: "string", description: "User message or question to ask the wiki assistant." },
          use_web_search: { type: "boolean", description: "Whether to allow the desktop app's configured web search provider." },
          use_any_txt_search: { type: "boolean", description: "Whether to allow the desktop app's configured AnyTXT provider." },
        },
        required: ["message"],
        additionalProperties: false,
      },
    },
    {
      name: "llm_wiki_graph",
      description: "Query the project knowledge graph through the desktop app API.",
      inputSchema: {
        type: "object",
        properties: {
          project_id: { type: "string", description: "Project UUID, project path, or 'current'. Defaults to current." },
          q: { type: "string", description: "Optional text filter." },
          node_type: { type: "string", description: "Optional node type filter." },
          limit: { type: "number", description: "Maximum nodes. The local API clamps to its configured maximum." },
        },
        additionalProperties: false,
      },
    },
    {
      name: "llm_wiki_rescan_sources",
      description: "Trigger the desktop app's source folder rescan for a project, using the user's Source Watch rules.",
      inputSchema: {
        type: "object",
        properties: {
          project_id: { type: "string", description: "Project UUID, project path, or 'current'. Defaults to current." },
        },
        additionalProperties: false,
      },
    },
    {
      name: "push_document",
      description: "Submit text content for review before adding to the wiki",
      inputSchema: {
        type: "object",
        properties: {
          path: { type: "string", description: "Target path relative to raw/sources, e.g. 'my-docs/report.md'" },
          content: { type: "string", description: "The text content to submit" },
          notes: { type: "string", description: "Optional notes about this content" },
        },
        required: ["path", "content"],
        additionalProperties: false,
      },
    },
    {
      name: "get_push_queue",
      description: "Get all pending items in the push review queue",
      inputSchema: {
        type: "object",
        properties: {},
        additionalProperties: false,
      },
    },
    {
      name: "approve_push",
      description: "Approve a push review item — it will be written to raw/sources and ingested",
      inputSchema: {
        type: "object",
        properties: {
          id: { type: "string", description: "The push item ID" },
        },
        required: ["id"],
        additionalProperties: false,
      },
    },
    {
      name: "reject_push",
      description: "Reject a push review item — it will be discarded",
      inputSchema: {
        type: "object",
        properties: {
          id: { type: "string", description: "The push item ID" },
        },
        required: ["id"],
        additionalProperties: false,
      },
    },
    {
      name: "update_push",
      description: "Update a push review item's content or review notes",
      inputSchema: {
        type: "object",
        properties: {
          id: { type: "string", description: "The push item ID" },
          content: { type: "string", description: "New content (optional)" },
          reviewNotes: { type: "string", description: "Reviewer notes (optional)" },
        },
        required: ["id"],
        additionalProperties: false,
      },
    },
  ],
}))

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const args = asObject(request.params.arguments ?? {})
  try {
    switch (request.params.name) {
      case "llm_wiki_status": {
        const [health, projects] = await Promise.all([
          client.health(),
          client.projects().catch(() => ({ projects: [], currentProject: null })),
        ])
        return textResult(JSON.stringify({ ...health, ...projects }, null, 2))
      }
      case "llm_wiki_projects": {
        await assertMcpEnabled()
        return textResult(JSON.stringify(await client.projects(), null, 2))
      }
      case "llm_wiki_files": {
        await assertMcpEnabled()
        const response = await client.files(projectId(args), {
          root: enumArg(args.root, ["wiki", "sources", "all"] as const, "wiki"),
          recursive: boolArg(args.recursive, true),
          maxFiles: numberArg(args.max_files),
        })
        return textResult(formatFileTree(response.files, response.truncated))
      }
      case "llm_wiki_read_file": {
        await assertMcpEnabled()
        const relPath = stringArg(args.path, "path")
        const { path, content } = await client.fileContent(projectId(args), relPath)
        return textResult(`# ${path}\n\n${truncateText(content, MAX_TEXT_BYTES)}`)
      }
      case "llm_wiki_search": {
        await assertMcpEnabled()
        const query = stringArg(args.query, "query")
        const search = await client.search(projectId(args), query, {
          topK: numberArg(args.top_k),
          includeContent: boolArg(args.include_content, false),
        })
        return textResult(formatSearchResults(query, search))
      }
      case "llm_wiki_chat": {
        await assertMcpEnabled()
        const message = stringArg(args.message, "message")
        const chat = await client.chat(projectId(args), message, {
          useWebSearch: boolArg(args.use_web_search, false),
          useAnyTxtSearch: boolArg(args.use_any_txt_search, false),
        })
        return textResult(formatChatResponse(message, chat))
      }
      case "llm_wiki_graph": {
        await assertMcpEnabled()
        const graph = await client.graph(projectId(args), {
          q: optionalStringArg(args.q),
          nodeType: optionalStringArg(args.node_type),
          limit: numberArg(args.limit),
        })
        return textResult(formatGraph(graph.nodes, graph.edges))
      }
      case "llm_wiki_rescan_sources": {
        await assertMcpEnabled()
        return textResult(JSON.stringify(await client.rescan(projectId(args)), null, 2))
      }
      case "push_document": {
        await assertMcpEnabled()
        const path = stringArg(args.path, "path")
        const content = stringArg(args.content, "content")
        const notes = optionalStringArg(args.notes)
        return textResult(JSON.stringify(await client.pushDocument(path, content, notes), null, 2))
      }
      case "get_push_queue": {
        await assertMcpEnabled()
        return textResult(JSON.stringify(await client.getPushQueue(), null, 2))
      }
      case "approve_push": {
        await assertMcpEnabled()
        const id = stringArg(args.id, "id")
        return textResult(JSON.stringify(await client.approvePush(id), null, 2))
      }
      case "reject_push": {
        await assertMcpEnabled()
        const id = stringArg(args.id, "id")
        return textResult(JSON.stringify(await client.rejectPush(id), null, 2))
      }
      case "update_push": {
        await assertMcpEnabled()
        const id = stringArg(args.id, "id")
        const content = optionalStringArg(args.content)
        const reviewNotes = optionalStringArg(args.reviewNotes)
        return textResult(JSON.stringify(await client.updatePush(id, content, reviewNotes), null, 2))
      }
      default:
        throw new McpError(ErrorCode.MethodNotFound, `Unknown tool: ${request.params.name}`)
    }
  } catch (err) {
    if (err instanceof McpError) throw err
    throw new McpError(
      ErrorCode.InternalError,
      err instanceof Error ? err.message : String(err),
    )
  }
})

async function assertMcpEnabled(): Promise<void> {
  const health = await client.health()
  if (health.mcpEnabled === false) {
    throw new McpError(
      ErrorCode.InvalidRequest,
      "LLM Wiki MCP access is disabled. Enable Settings -> API + MCP -> Enable MCP access in the desktop app.",
    )
  }
}

function textResult(text: string) {
  return {
    content: [{ type: "text" as const, text }],
  }
}

function asObject(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {}
  return value as Record<string, unknown>
}

function projectId(args: Record<string, unknown>): string {
  return optionalStringArg(args.project_id) ?? DEFAULT_PROJECT_ID
}

function stringArg(value: unknown, name: string): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new McpError(ErrorCode.InvalidParams, `${name} is required`)
  }
  return value
}

function optionalStringArg(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() !== "" ? value : undefined
}

function boolArg(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback
}

function numberArg(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined
}

function enumArg<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return typeof value === "string" && allowed.includes(value as T) ? value as T : fallback
}

function truncateText(value: string, maxBytes: number): string {
  const bytes = Buffer.byteLength(value, "utf8")
  if (bytes <= maxBytes) return value
  let out = ""
  let used = 0
  for (const ch of value) {
    const size = Buffer.byteLength(ch, "utf8")
    if (used + size > maxBytes) break
    out += ch
    used += size
  }
  return `${out}\n\n[truncated: ${bytes - used} bytes omitted]`
}

function formatFileTree(files: ApiFileNode[], truncated = false): string {
  if (files.length === 0) return "No files found."
  const lines: string[] = truncated
    ? ["[warning] File tree was truncated by the LLM Wiki API maxFiles limit.", ""]
    : []
  const walk = (nodes: ApiFileNode[], depth: number) => {
    for (const node of nodes) {
      const prefix = "  ".repeat(depth)
      lines.push(`${prefix}${node.isDir ? "📁" : "📄"} ${node.path}`)
      if (node.children) walk(node.children, depth + 1)
    }
  }
  walk(files, 0)
  return lines.join("\n")
}

function formatSearchResults(query: string, search: { results: ApiSearchResult[]; mode?: string; tokenHits?: number; vectorHits?: number }): string {
  const { results } = search
  if (results.length === 0) return `No results for "${query}".`
  const meta = [
    search.mode ? `Mode: ${search.mode}` : null,
    typeof search.tokenHits === "number" ? `Token hits: ${search.tokenHits}` : null,
    typeof search.vectorHits === "number" ? `Vector hits: ${search.vectorHits}` : null,
  ].filter(Boolean)
  const lines = [`# Search results for "${query}"`, ...(meta.length > 0 ? [meta.join(" | ")] : []), ""]
  results.forEach((result, index) => {
    lines.push(`## ${index + 1}. ${result.title}`)
    lines.push(`Path: ${result.path}`)
    lines.push(`Score: ${result.score.toFixed(6)}${typeof result.vectorScore === "number" ? ` | Vector score: ${result.vectorScore.toFixed(6)}` : ""}`)
    if (result.snippet) lines.push(`Snippet: ${result.snippet}`)
    if (result.images && result.images.length > 0) {
      lines.push(`Images: ${result.images.map((image) => image.url).join(", ")}`)
    }
    lines.push("")
  })
  return lines.join("\n")
}

function formatGraph(nodes: ApiGraphNode[], edges: Array<{ source: string; target: string; weight?: number }>): string {
  const typeCounts = new Map<string, number>()
  for (const node of nodes) typeCounts.set(node.type, (typeCounts.get(node.type) ?? 0) + 1)
  const lines = [
    "# Knowledge graph",
    "",
    `Nodes: ${nodes.length}`,
    `Edges: ${edges.length}`,
    "",
    "## Node types",
    ...[...typeCounts.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([type, count]) => `- ${type}: ${count}`),
    "",
    "## Top nodes",
    ...nodes
      .slice()
      .sort((a, b) => (b.linkCount ?? 0) - (a.linkCount ?? 0))
      .slice(0, 30)
      .map((node) => `- ${node.label} (${node.type}, ${node.linkCount ?? 0} links)${node.path ? ` — ${node.path}` : ""}`),
  ]
  return lines.join("\n")
}

function formatChatResponse(
  message: string,
  chat: { response: string; references: ApiChatReference[]; warnings?: string[] },
): string {
  const lines = [
    `# Chat response`,
    "",
    `Question: ${message}`,
    "",
    chat.response || "(empty response)",
  ]

  if (chat.references.length > 0) {
    lines.push("", "## References")
    for (const [index, ref] of chat.references.entries()) {
      const extras = [
        ref.kind ? `kind=${ref.kind}` : null,
        ref.source ? `source=${ref.source}` : null,
        ref.url ? `url=${ref.url}` : null,
      ].filter(Boolean)
      lines.push(`- [${index + 1}] ${ref.title} - ${ref.path}${extras.length > 0 ? ` (${extras.join(", ")})` : ""}`)
      if (ref.snippet) {
        lines.push(`  Snippet: ${truncateText(ref.snippet, 800)}`)
      }
    }
  }

  if (chat.warnings && chat.warnings.length > 0) {
    lines.push("", "## Warnings", ...chat.warnings.map((warning) => `- ${warning}`))
  }

  return lines.join("\n")
}

async function main(): Promise<void> {
  const transport = new StdioServerTransport()
  await server.connect(transport)
  console.error(`LLM Wiki MCP server v${VERSION} connected to ${process.env.LLM_WIKI_API_BASE_URL ?? "http://127.0.0.1:19828"}`)
}

main().catch((err) => {
  console.error("Failed to start LLM Wiki MCP server:", err)
  process.exit(1)
})
