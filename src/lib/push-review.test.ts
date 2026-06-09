import { describe, it, expect, vi } from "vitest"

vi.mock("@/commands/fs", () => ({
  readFile: vi.fn(),
  writeFile: vi.fn(),
  fileExists: vi.fn(),
  writePushSource: vi.fn(),
}))

import { loadQueue, generateId } from "./push-review"
import { readFile } from "@/commands/fs"

const mockReadFile = vi.mocked(readFile)

describe("push-review persistence", () => {
  it("loads empty queue when file does not exist", async () => {
    mockReadFile.mockRejectedValue(new Error("ENOENT"))
    const items = await loadQueue("/nonexistent/project", "proj-1")
    expect(items).toEqual([])
  })

  it("generates correct id format", () => {
    const id = generateId()
    expect(id).toMatch(/^push-\d+-[a-z0-9]+$/)
  })
})