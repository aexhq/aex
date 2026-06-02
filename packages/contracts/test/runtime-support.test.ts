import { describe, expect, it } from "vitest";
import { checkRuntimeSupported } from "../src/index.js";

describe("checkRuntimeSupported - managed-only runtime spec", () => {
  it("accepts an absent runtime for every provider", () => {
    expect(checkRuntimeSupported("anthropic", undefined)).toEqual({ ok: true });
    expect(checkRuntimeSupported("deepseek", undefined)).toEqual({ ok: true });
  });

  it("accepts managed for every provider", () => {
    expect(checkRuntimeSupported("anthropic", "managed")).toEqual({ ok: true });
    expect(checkRuntimeSupported("deepseek", "managed")).toEqual({ ok: true });
    expect(checkRuntimeSupported("openai", "managed")).toEqual({ ok: true });
    expect(checkRuntimeSupported("gemini", "managed")).toEqual({ ok: true });
    expect(checkRuntimeSupported("mistral", "managed")).toEqual({ ok: true });
  });
});
