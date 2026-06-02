import { describe, expect, it } from "vitest";
import { checkRuntimeSupported } from "../src/index.js";

describe("checkRuntimeSupported — centralized native-first runtime spec", () => {
  it("accepts an absent runtime (dispatcher auto-routes native-first then managed)", () => {
    expect(checkRuntimeSupported("anthropic", undefined)).toEqual({ ok: true });
    expect(checkRuntimeSupported("deepseek", undefined)).toEqual({ ok: true });
  });

  it("accepts native for a native-capable provider", () => {
    expect(checkRuntimeSupported("anthropic", "native")).toEqual({ ok: true });
  });

  it("accepts managed for any provider", () => {
    expect(checkRuntimeSupported("anthropic", "managed")).toEqual({ ok: true });
    expect(checkRuntimeSupported("deepseek", "managed")).toEqual({ ok: true });
  });

  it("rejects native for a provider with no native runtime, with a diagnostic code + message", () => {
    const result = checkRuntimeSupported("deepseek", "native");
    expect(result.ok).toBe(false);
    expect(result.code).toBe("runtime_native_unsupported");
    expect(result.message).toContain("native");
    expect(result.message).toContain("deepseek");
    expect(result.message).toContain("anthropic");
  });
});
