import { describe, expect, it, vi } from "vitest"
import type { ChatRuntimeConfig } from "./chat-runtime-config"

const { mockSearchWiki, mockReadFile, mockBuildRetrievalGraph, mockGetRelatedNodes, mockWebSearch, mockAnyTxtSearchSmart } = vi.hoisted(() => ({
  mockSearchWiki: vi.fn(),
  mockReadFile: vi.fn(),
  mockBuildRetrievalGraph: vi.fn(),
  mockGetRelatedNodes: vi.fn(),
  mockWebSearch: vi.fn(),
  mockAnyTxtSearchSmart: vi.fn(),
}))

vi.mock("@/lib/search", () => ({
  searchWiki: mockSearchWiki,
  tokenizeQuery: vi.fn((query: string) => query.toLowerCase().split(/\s+/).filter(Boolean)),
}))

vi.mock("@/commands/fs", () => ({
  readFile: mockReadFile,
}))

vi.mock("@/lib/graph-relevance", () => ({
  buildRetrievalGraph: mockBuildRetrievalGraph,
  getRelatedNodes: mockGetRelatedNodes,
}))

vi.mock("@/lib/web-search", () => ({
  resolveSearchConfig: vi.fn((config) => config),
  webSearch: mockWebSearch,
}))

vi.mock("@/lib/anytxt-search", () => ({
  anyTxtSearchSmart: mockAnyTxtSearchSmart,
}))

import { buildChatRetrievalContext } from "./chat-retrieval"

function makeConfig(): ChatRuntimeConfig {
  return {
    llmConfig: {
      provider: "openai",
      apiKey: "k",
      model: "gpt-4.1-mini",
      ollamaUrl: "http://localhost:11434",
      customEndpoint: "",
      maxContextSize: 204800,
      reasoning: { mode: "auto" },
    },
    searchApiConfig: {
      provider: "none",
      apiKey: "",
      providerConfigs: {},
      deepResearchSource: "web",
      anyTxt: {
        enabled: false,
        endpoint: "http://127.0.0.1:9920",
        filterDir: "",
        filterExt: "*",
        limit: 20,
      },
    },
    embeddingConfig: { enabled: false, endpoint: "", apiKey: "", model: "" },
    outputLanguage: "English",
    dataVersion: 1,
  }
}

describe("buildChatRetrievalContext", () => {
  function installDefaultMocks(): void {
    mockSearchWiki.mockResolvedValue([
      {
        path: "/tmp/project/wiki/concepts/rag.md",
        title: "RAG",
        snippet: "Retrieval augmented generation",
        titleMatch: true,
        score: 10,
        images: [],
      },
    ])
    mockReadFile.mockImplementation(async (path: string) => {
      if (path.endsWith("/purpose.md")) return "Answer wiki questions accurately."
      if (path.endsWith("/wiki/index.md")) return "## Concepts\n- [[RAG]]"
      if (path.endsWith("/wiki/concepts/rag.md")) return "# RAG\n\nRAG stands for retrieval augmented generation."
      throw new Error(`unexpected path: ${path}`)
    })
    mockBuildRetrievalGraph.mockResolvedValue({ nodes: new Map(), dataVersion: 1 })
    mockGetRelatedNodes.mockReturnValue([])
    mockWebSearch.mockResolvedValue([])
    mockAnyTxtSearchSmart.mockResolvedValue([])
  }

  it("builds wiki retrieval context and references from search hits", async () => {
    installDefaultMocks()

    const context = await buildChatRetrievalContext({
      projectPath: "/tmp/project",
      projectName: "Demo",
      message: "What is RAG?",
      useWebSearch: false,
      useAnyTxtSearch: false,
      config: makeConfig(),
    })

    expect(context.purpose).toBe("Answer wiki questions accurately.")
    expect(context.index).toContain("[[RAG]]")
    expect(context.wikiPages).toEqual([
      expect.objectContaining({
        id: "wiki/concepts/rag",
        title: "RAG",
        path: "wiki/concepts/rag.md",
        priority: 0,
      }),
    ])
    expect(context.references).toEqual([
      expect.objectContaining({
        title: "RAG",
        path: "wiki/concepts/rag.md",
        kind: "wiki",
      }),
    ])
    expect(context.externalResults).toEqual([])
    expect(context.warnings).toEqual([])
  })

  it("uses folder-aware page ids so same basenames do not collide", async () => {
    mockSearchWiki.mockResolvedValue([
      {
        path: "/tmp/project/wiki/concepts/overview.md",
        title: "Concept Overview",
        snippet: "Concept details",
        titleMatch: true,
        score: 10,
        images: [],
      },
      {
        path: "/tmp/project/wiki/guides/overview.md",
        title: "Guide Overview",
        snippet: "Guide details",
        titleMatch: false,
        score: 8,
        images: [],
      },
    ])
    mockReadFile.mockImplementation(async (path: string) => {
      if (path.endsWith("/purpose.md")) return ""
      if (path.endsWith("/wiki/index.md")) return "## Overviews"
      if (path.endsWith("/wiki/concepts/overview.md")) return "# Concept Overview"
      if (path.endsWith("/wiki/guides/overview.md")) return "# Guide Overview"
      throw new Error(`unexpected path: ${path}`)
    })
    mockBuildRetrievalGraph.mockResolvedValue({ nodes: new Map(), dataVersion: 1 })
    mockGetRelatedNodes.mockReturnValue([])
    mockWebSearch.mockResolvedValue([])
    mockAnyTxtSearchSmart.mockResolvedValue([])

    const context = await buildChatRetrievalContext({
      projectPath: "/tmp/project",
      projectName: "Demo",
      message: "overview",
      useWebSearch: false,
      useAnyTxtSearch: false,
      config: makeConfig(),
    })

    expect(context.wikiPages).toHaveLength(2)
    expect(context.wikiPages[0]?.id).not.toBe(context.wikiPages[1]?.id)
  })

  it("keeps an oversized page under tiny budgets by fitting truncation inside the limit", async () => {
    mockSearchWiki.mockResolvedValue([
      {
        path: "/tmp/project/wiki/concepts/rag.md",
        title: "RAG",
        snippet: "Retrieval augmented generation",
        titleMatch: true,
        score: 10,
        images: [],
      },
    ])
    mockReadFile.mockImplementation(async (path: string) => {
      if (path.endsWith("/purpose.md")) return ""
      if (path.endsWith("/wiki/index.md")) return "RAG"
      if (path.endsWith("/wiki/concepts/rag.md")) return "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
      throw new Error(`unexpected path: ${path}`)
    })
    mockBuildRetrievalGraph.mockResolvedValue({ nodes: new Map(), dataVersion: 1 })
    mockGetRelatedNodes.mockReturnValue([])
    mockWebSearch.mockResolvedValue([])
    mockAnyTxtSearchSmart.mockResolvedValue([])

    const context = await buildChatRetrievalContext({
      projectPath: "/tmp/project",
      projectName: "Demo",
      message: "RAG",
      useWebSearch: false,
      useAnyTxtSearch: false,
      config: {
        ...makeConfig(),
        llmConfig: {
          ...makeConfig().llmConfig,
          maxContextSize: 40,
        },
      },
    })

    expect(context.wikiPages).toHaveLength(1)
    expect(context.wikiPages[0]?.content.length).toBeLessThanOrEqual(20)
  })

  it("uses the graph node whose path matches the search hit when basenames are shared", async () => {
    mockSearchWiki.mockResolvedValue([
      {
        path: "/tmp/project/wiki/guides/overview.md",
        title: "Guide Overview",
        snippet: "Guide details",
        titleMatch: true,
        score: 10,
        images: [],
      },
    ])
    mockReadFile.mockImplementation(async (path: string) => {
      if (path.endsWith("/purpose.md")) return ""
      if (path.endsWith("/wiki/index.md")) return "## Overviews"
      if (path.endsWith("/wiki/guides/overview.md")) return "# Guide Overview"
      if (path.endsWith("/wiki/guides/guide-child.md")) return "# Guide Child"
      if (path.endsWith("/wiki/concepts/concept-child.md")) return "# Concept Child"
      throw new Error(`unexpected path: ${path}`)
    })
    mockBuildRetrievalGraph.mockResolvedValue({
      dataVersion: 1,
      nodes: new Map([
        [
          "concepts-overview",
          {
            id: "concepts-overview",
            title: "Concept Overview",
            type: "concept",
            path: "/tmp/project/wiki/concepts/overview.md",
            sources: [],
            outLinks: new Set(),
            inLinks: new Set(),
          },
        ],
        [
          "guides-overview",
          {
            id: "guides-overview",
            title: "Guide Overview",
            type: "guide",
            path: "/tmp/project/wiki/guides/overview.md",
            sources: [],
            outLinks: new Set(),
            inLinks: new Set(),
          },
        ],
      ]),
    })
    mockGetRelatedNodes.mockImplementation((nodeId: string) => {
      if (nodeId === "guides-overview") {
        return [
          {
            node: {
              id: "guide-child",
              title: "Guide Child",
              type: "guide",
              path: "/tmp/project/wiki/guides/guide-child.md",
              sources: [],
              outLinks: new Set(),
              inLinks: new Set(),
            },
            relevance: 3,
          },
        ]
      }

      if (nodeId === "overview") {
        return [
          {
            node: {
              id: "concept-child",
              title: "Concept Child",
              type: "concept",
              path: "/tmp/project/wiki/concepts/concept-child.md",
              sources: [],
              outLinks: new Set(),
              inLinks: new Set(),
            },
            relevance: 3,
          },
        ]
      }

      return []
    })
    mockWebSearch.mockResolvedValue([])
    mockAnyTxtSearchSmart.mockResolvedValue([])

    const context = await buildChatRetrievalContext({
      projectPath: "/tmp/project",
      projectName: "Demo",
      message: "overview",
      useWebSearch: false,
      useAnyTxtSearch: false,
      config: makeConfig(),
    })

    expect(context.wikiPages.map((page) => page.path)).toContain("wiki/guides/guide-child.md")
    expect(context.wikiPages.map((page) => page.path)).not.toContain("wiki/concepts/concept-child.md")
  })
})
