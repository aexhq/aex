import { describe, expect, it } from "vitest";
import { checkRuntimeSupported, parseRuntimeKind } from "../src/index.js";

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

// checkRuntimeSupported is a managed-only no-op; the managed-only contract is
// actually enforced one layer down by the schema parser, which rejects any
// runtime string other than "managed" (notably the retired "native"). These
// tests fail if that rejection ever regresses.
describe("parseRuntimeKind - managed-only schema enforcement", () => {
  it("accepts an absent runtime and managed", () => {
    expect(parseRuntimeKind(undefined)).toBeUndefined();
    expect(parseRuntimeKind("managed")).toBe("managed");
  });

  it("rejects native and any other runtime string", () => {
    expect(() => parseRuntimeKind("native")).toThrow(/runtime must be one of: managed/);
    expect(() => parseRuntimeKind("byo")).toThrow(/runtime must be one of: managed/);
  });
});
