import type { ChatMessage as LLMMessage } from "@/lib/llm-client"
import type { ChatRetrievedContext } from "./chat-retrieval"

export interface BuildChatPromptMessagesInput {
  projectName: string
  message: string
  history: LLMMessage[]
  outputLanguage: string
  retrieval: ChatRetrievedContext
}

function formatExternalSearchContext(retrieval: ChatRetrievedContext): string {
  if (retrieval.externalResults.length === 0) return ""

  return retrieval.externalResults
    .map((result, index) => [
      `### [E${index + 1}] ${result.title}`,
      `Source: ${result.source}`,
      `URL: ${result.url}`,
      "",
      result.snippet,
    ].join("\n"))
    .join("\n\n---\n\n")
}

function formatCodeContext(retrieval: ChatRetrievedContext): string {
  if (!retrieval.codeContext) return ""

  const snippets = retrieval.codeContext.snippets
    .map((snippet, index) => [
      `### [C${index + 1}] ${snippet.symbolName}`,
      `Path: ${snippet.filePath}:${snippet.startLine}`,
      `Language: ${snippet.language}`,
      `Reason: ${snippet.reason}`,
      "",
      "```",
      snippet.content,
      "```",
    ].join("\n"))
    .join("\n\n---\n\n")

  const relationships = retrieval.codeContext.relationships.length > 0
    ? [
        "## Code Relationships",
        ...retrieval.codeContext.relationships.map((rel) => (
          `- ${rel.type}: ${rel.source} -> ${rel.target} (${rel.sourcePath}:${rel.line})`
        )),
      ].join("\n")
    : ""

  return [snippets, relationships].filter(Boolean).join("\n\n")
}

export function buildChatPromptMessages(
  input: BuildChatPromptMessagesInput,
): LLMMessage[] {
  const pageList = input.retrieval.wikiPages
    .map((page, index) => `[${index + 1}] ${page.title} (${page.path})`)
    .join("\n")

  const pagesContext = input.retrieval.wikiPages.length > 0
    ? input.retrieval.wikiPages
      .map((page, index) => `### [${index + 1}] ${page.title}\nPath: ${page.path}\n\n${page.content}`)
      .join("\n\n---\n\n")
    : "(No wiki pages found)"

  const externalContext = formatExternalSearchContext(input.retrieval)
  const codeContext = formatCodeContext(input.retrieval)
  const providedSourceNames = [
    input.retrieval.wikiPages.length > 0 ? "numbered wiki pages" : "",
    codeContext ? "code snippets" : "",
    externalContext ? "external sources" : "",
  ].filter(Boolean).join(", ")

  return [
    {
      role: "system",
      content: [
        `You are a knowledgeable wiki assistant for the project "${input.projectName}". Answer questions based on the provided project context below.`,
        "",
        "## Rules",
        providedSourceNames
          ? `- Answer based ONLY on the provided ${providedSourceNames}.`
          : "- Answer only if the provided context is sufficient.",
        "- If the provided sources do not contain enough information, say so honestly.",
        "- Use [[wikilink]] syntax to reference wiki pages.",
        codeContext
          ? "- When citing code, use code source IDs like [C1], [C2] and mention file paths when useful."
          : "",
        externalContext
          ? "- When citing wiki information, use page numbers like [1], [2]. When citing external information, use external source IDs like [E1], [E2]."
          : "- Use the page number in brackets when citing wiki information, e.g. [1], [2].",
        "- At the VERY END of your response, add a hidden comment listing which wiki page numbers you used:",
        "  <!-- cited: 1, 3, 5 -->",
        "",
        "Use markdown formatting for clarity.",
        "",
        input.retrieval.purpose ? `## Wiki Purpose\n${input.retrieval.purpose}` : "",
        input.retrieval.index ? `## Wiki Index\n${input.retrieval.index}` : "",
        pageList ? `## Page List\n${pageList}` : "",
        `## Wiki Pages\n\n${pagesContext}`,
        codeContext ? `## Code Analysis Context\n\n${codeContext}` : "",
        externalContext ? `## External Sources\n\n${externalContext}` : "",
        input.retrieval.warnings.length > 0
          ? `## External Source Errors\n${input.retrieval.warnings.map((warning) => `- ${warning}`).join("\n")}`
          : "",
        "",
        "---",
        "",
        `## MANDATORY OUTPUT LANGUAGE: ${input.outputLanguage}`,
        "",
        `You MUST write your entire response in ${input.outputLanguage}.`,
      ].filter(Boolean).join("\n"),
    },
    ...input.history,
    {
      role: "user",
      content: input.message,
    },
  ]
}
