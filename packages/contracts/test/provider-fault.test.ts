import { describe, expect, it } from "vitest";
import {
  isKnownProviderFaultKind,
  parseProviderFault,
  type ProviderFault
} from "../src/index.js";

describe("canonical ProviderFault", () => {
  it("parses every exact known kind and canonical optional fields", () => {
    for (const kind of ["rate_limit", "overloaded", "quota_exceeded", "unavailable", "provider_error"] as const) {
      expect(parseProviderFault({ kind })).toEqual({ kind });
      expect(isKnownProviderFaultKind(kind)).toBe(true);
    }
    const fault: ProviderFault = parseProviderFault({
      provider: "anthropic",
      kind: "rate_limit",
      status: 429,
      retryAfterMs: 2_000,
      message: "request throttled"
    });
    expect(fault).toEqual({
      provider: "anthropic",
      kind: "rate_limit",
      status: 429,
      retryAfterMs: 2_000,
      message: "request throttled"
    });
  });

  it("preserves a valid future kind without treating arbitrary producer strings as known", () => {
    const fault = parseProviderFault({ kind: "capacity_window_v2", status: 503 });
    expect(fault).toEqual({ kind: "capacity_window_v2", status: 503 });
    expect(isKnownProviderFaultKind(fault.kind)).toBe(false);
  });

  it.each([
    [{ kind: "Rate_Limit" }],
    [{ kind: "rate_limit", statusCode: 429 }],
    [{ kind: "rate_limit", retry_after_ms: 1_000 }],
    [{ kind: "rate_limit", status: "429" }],
    [{ kind: "rate_limit", provider: undefined }],
    [{ kind: "rate_limit", retryAfterMs: -1 }],
    [{ kind: "rate_limit", message: "" }],
    [{ kind: "rate_limit", message: "x".repeat(513) }],
    [{ kind: "rate_limit", extra: true }]
  ])("rejects aliases, coercions, or malformed present fields: %j", (value) => {
    expect(() => parseProviderFault(value)).toThrow(/providerFault/);
  });
});
