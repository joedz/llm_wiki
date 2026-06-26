import { describe, expect, it } from "vitest"
import { buildChatPromptMessages } from "./chat-prompt-builder"

describe("buildChatPromptMessages", () => {
  it("produces a citation-aware system prompt and preserves history before the final user turn", () => {
    const messages = buildChatPromptMessages({
      projectName: "Demo",
      message: "What is RAG?",
      history: [
        { role: "user", content: "Earlier question" },
        { role: "assistant", content: "Earlier answer [1]." },
      ],
      outputLanguage: "English",
      retrieval: {
        purpose: "Answer wiki questions accurately.",
        index: "## Concepts\n- [[RAG]]",
        wikiPages: [
          {
            id: "rag",
            title: "RAG",
            path: "wiki/concepts/rag.md",
            content: "# RAG\n\nRAG stands for retrieval augmented generation.",
            priority: 0,
          },
        ],
        codeContext: null,
        externalResults: [],
        references: [
          {
            title: "RAG",
            path: "wiki/concepts/rag.md",
            kind: "wiki",
            snippet: "RAG stands for retrieval augmented generation.",
          },
        ],
        warnings: [],
      },
    })

    expect(messages[0].role).toBe("system")
    expect(messages[0].content).toContain("Use the page number in brackets")
    expect(messages[0].content).toContain("## Wiki Purpose")
    expect(messages[0].content).toContain("## Page List")
    expect(messages[1]).toEqual({ role: "user", content: "Earlier question" })
    expect(messages[2]).toEqual({ role: "assistant", content: "Earlier answer [1]." })
    expect(messages[messages.length - 1]).toEqual({
      role: "user",
      content: "What is RAG?",
    })
  })

  it("includes external source instructions, source blocks, and warnings when present", () => {
    const messages = buildChatPromptMessages({
      projectName: "Demo",
      message: "What is new about RAG?",
      history: [],
      outputLanguage: "English",
      retrieval: {
        purpose: "",
        index: "",
        wikiPages: [],
        codeContext: null,
        externalResults: [
          {
            title: "RAG News",
            url: "https://example.com/rag",
            snippet: "Latest RAG developments.",
            source: "example.com",
          },
        ],
        references: [
          {
            title: "RAG News",
            path: "https://example.com/rag",
            kind: "external",
            url: "https://example.com/rag",
            source: "example.com",
            snippet: "Latest RAG developments.",
          },
        ],
        warnings: ["Web Search: timed out"],
      },
    })

    expect(messages[0].content).toContain("external source IDs like [E1], [E2]")
    expect(messages[0].content).toContain("## External Sources")
    expect(messages[0].content).toContain("### [E1] RAG News")
    expect(messages[0].content).toContain("## External Source Errors")
    expect(messages[0].content).toContain("- Web Search: timed out")
  })

  it("includes code-analysis context separately from wiki pages", () => {
    const messages = buildChatPromptMessages({
      projectName: "Demo",
      message: "谁调用了 runPlan",
      history: [],
      outputLanguage: "Chinese",
      retrieval: {
        purpose: "",
        index: "",
        wikiPages: [],
        codeContext: {
          snippets: [
            {
              filePath: "raw/code/demo/app.ts",
              symbolName: "runPlan",
              language: "TypeScript",
              content: "export function runPlan() {\n  return buildAnswer()\n}",
              startLine: 1,
              endLine: 3,
              reason: "symbol-match",
            },
          ],
          relationships: [
            {
              type: "calls",
              source: "handleChat",
              target: "runPlan",
              sourcePath: "raw/code/demo/routes.ts",
              targetPath: "raw/code/demo/app.ts",
              line: 4,
            },
          ],
          references: [
            {
              title: "runPlan",
              path: "raw/code/demo/app.ts",
              kind: "code",
              snippet: "export function runPlan()",
            },
          ],
        },
        externalResults: [],
        references: [
          {
            title: "runPlan",
            path: "raw/code/demo/app.ts",
            kind: "code",
            snippet: "export function runPlan()",
          },
        ],
        warnings: [],
      },
    })

    expect(messages[0].content).toContain("## Code Analysis Context")
    expect(messages[0].content).toContain("### [C1] runPlan")
    expect(messages[0].content).toContain("calls: handleChat -> runPlan")
  })
})
