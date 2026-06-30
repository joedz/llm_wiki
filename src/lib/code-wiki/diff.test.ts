import { describe, expect, it } from "vitest"
import { isOverlayInteresting, type DiffOverlay } from "./diff"

function sampleOverlay(over: Partial<DiffOverlay> = {}): DiffOverlay {
  return {
    version: "1.0.0",
    baseBranch: "HEAD",
    generatedAt: "2026-06-30T00:00:00.000Z",
    changedFiles: [],
    changedNodeIds: [],
    affectedNodeIds: [],
    warnings: [],
    ...over,
  }
}

describe("isOverlayInteresting", () => {
  it("returns false for null", () => {
    expect(isOverlayInteresting(null)).toBe(false);
  })

  it("returns false for empty overlay", () => {
    expect(isOverlayInteresting(sampleOverlay())).toBe(false);
  })

  it("returns true when changedNodeIds is non-empty", () => {
    expect(
      isOverlayInteresting(sampleOverlay({ changedNodeIds: ["file:src/foo.ts"] })),
    ).toBe(true);
  })

  it("returns true when affectedNodeIds is non-empty", () => {
    expect(
      isOverlayInteresting(sampleOverlay({ affectedNodeIds: ["function:src/bar.ts:f"] })),
    ).toBe(true);
  })
})
