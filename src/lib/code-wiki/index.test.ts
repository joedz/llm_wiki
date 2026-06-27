import { describe, expect, it } from "vitest"
import { RAW_CODE_ROOT, WIKI_CODE_ROOT } from "./index"

describe("code-wiki module entry", () => {
  it("exports the expected root constants", () => {
    expect(RAW_CODE_ROOT).toBe("raw/code")
    expect(WIKI_CODE_ROOT).toBe("wiki/code_wiki")
  })
})