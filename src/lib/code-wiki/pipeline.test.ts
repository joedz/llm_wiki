import { describe, expect, it } from "vitest"
import { hasLlmConfig, llmSpecFromConfig } from "./pipeline"

describe("llmSpecFromConfig", () => {
  it("returns null when config is missing", () => {
    expect(llmSpecFromConfig(null)).toBeNull()
    expect(llmSpecFromConfig(undefined)).toBeNull()
  })

  it("returns null when api key is missing for non-ollama providers", () => {
    expect(
      llmSpecFromConfig({ provider: "anthropic", model: "claude-3-5-sonnet" }),
    ).toBeNull()
  })

  it("returns null when model is missing", () => {
    expect(llmSpecFromConfig({ provider: "anthropic", apiKey: "sk-abc" })).toBeNull()
  })

  it("maps known providers correctly", () => {
    const anthropic = llmSpecFromConfig({
      provider: "anthropic",
      apiKey: "sk-abc",
      model: "claude-3-5-sonnet",
    })
    expect(anthropic?.provider).toBe("anthropic")
    expect(anthropic?.apiKey).toBe("sk-abc")

    const ollama = llmSpecFromConfig({
      provider: "ollama",
      apiKey: "k",
      model: "llama3",
      ollamaUrl: "http://localhost:11434",
    })
    expect(ollama?.provider).toBe("ollama")
    expect(ollama?.baseUrl).toBe("http://localhost:11434")
  })

  it("falls back to openai for unknown providers", () => {
    const spec = llmSpecFromConfig({
      provider: "azure",
      apiKey: "k",
      model: "gpt-4",
    })
    expect(spec?.provider).toBe("openai")
  })

  it("uses customEndpoint as baseUrl for custom providers", () => {
    const spec = llmSpecFromConfig({
      provider: "custom",
      apiKey: "k",
      model: "x",
      customEndpoint: "https://api.example.com/v1",
    })
    expect(spec?.baseUrl).toBe("https://api.example.com/v1")
  })
})

describe("hasLlmConfig", () => {
  it("returns false for null", () => {
    expect(hasLlmConfig(null)).toBe(false)
  })
  it("returns true for a valid spec", () => {
    expect(
      hasLlmConfig({ provider: "anthropic", apiKey: "k", model: "claude" }),
    ).toBe(true)
  })
})
