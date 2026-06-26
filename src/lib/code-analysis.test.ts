import { describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  listDirectory: vi.fn(),
  readFile: vi.fn(),
}))

vi.mock("@/commands/fs", () => ({
  listDirectory: mocks.listDirectory,
  readFile: mocks.readFile,
}))

import { buildCodeAnalysisContext, isCodeSourcePath } from "./code-analysis"

describe("code-analysis", () => {
  it("recognizes raw/code paths as code sources", () => {
    expect(isCodeSourcePath("/project/raw/code/app/src/main.ts")).toBe(true)
    expect(isCodeSourcePath("/project/raw/sources/app/src/main.ts")).toBe(false)
  })

  it("builds caller context for natural-language code questions", async () => {
    mocks.listDirectory.mockResolvedValue([
      {
        name: "app.ts",
        path: "/project/raw/code/demo/app.ts",
        is_dir: false,
      },
      {
        name: "routes.ts",
        path: "/project/raw/code/demo/routes.ts",
        is_dir: false,
      },
    ])
    mocks.readFile.mockImplementation(async (path: string) => {
      if (path.endsWith("/app.ts")) {
        return [
          "export function runPlan() {",
          "  return buildAnswer()",
          "}",
          "",
          "function buildAnswer() {",
          "  return 'ok'",
          "}",
        ].join("\n")
      }
      if (path.endsWith("/routes.ts")) {
        return [
          "import { runPlan } from './app'",
          "",
          "export function handleChat() {",
          "  return runPlan()",
          "}",
        ].join("\n")
      }
      throw new Error(`unexpected path: ${path}`)
    })

    const context = await buildCodeAnalysisContext({
      projectPath: "/project",
      message: "谁调用了 runPlan",
      maxContextSize: 204800,
    })

    expect(context).not.toBeNull()
    expect(context?.snippets.map((snippet) => snippet.symbolName)).toContain("runPlan")
    expect(context?.relationships).toEqual([
      expect.objectContaining({
        type: "calls",
        source: "handleChat",
        target: "runPlan",
      }),
    ])
    expect(context?.references).toEqual([
      expect.objectContaining({
        kind: "code",
        path: "raw/code/demo/app.ts",
      }),
      expect.objectContaining({
        kind: "code",
        path: "raw/code/demo/routes.ts",
      }),
    ])
  })

  it("builds overview context for broad code explanation questions", async () => {
    mocks.listDirectory.mockResolvedValue([
      {
        name: "main.ts",
        path: "/project/raw/code/tool/main.ts",
        is_dir: false,
      },
      {
        name: "worker.ts",
        path: "/project/raw/code/tool/worker.ts",
        is_dir: false,
      },
    ])
    mocks.readFile.mockImplementation(async (path: string) => {
      if (path.endsWith("/main.ts")) {
        return [
          "import { runWorker } from './worker'",
          "",
          "export function main() {",
          "  return runWorker()",
          "}",
        ].join("\n")
      }
      if (path.endsWith("/worker.ts")) {
        return [
          "export function runWorker() {",
          "  return 'done'",
          "}",
        ].join("\n")
      }
      throw new Error(`unexpected path: ${path}`)
    })

    const context = await buildCodeAnalysisContext({
      projectPath: "/project",
      message: "这个方案是干什么的",
      maxContextSize: 204800,
    })

    expect(context).not.toBeNull()
    expect(context?.snippets.map((snippet) => snippet.reason)).toContain("project-overview")
    expect(context?.references).toEqual([
      expect.objectContaining({
        kind: "code",
        path: "raw/code/tool/main.ts",
      }),
      expect.objectContaining({
        kind: "code",
        path: "raw/code/tool/worker.ts",
      }),
    ])
  })
})
