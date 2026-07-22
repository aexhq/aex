import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

function source(relative: string): string {
  return readFileSync(new URL(`../../src/${relative}`, import.meta.url), "utf8");
}

describe("provider-fault ownership", () => {
  it("keeps canonical Session consumption typed and heuristic-free", () => {
    const client = source("client.ts");
    expect(client).toContain("session.providerFault ?? legacySessionProviderFault(session, debug)");
    expect(client).not.toMatch(/session as \{ readonly providerFault/);
    expect(client).not.toMatch(/session as \{ readonly error/);
    expect(client).not.toContain("faultFromErrorMessage");
    expect(client).not.toMatch(/rate\.\?limit|too many requests/);
  });

  it("keeps raw-provider compatibility exact instead of substring-classifying", () => {
    const retry = source("retry.ts");
    expect(retry).toContain("LEGACY_FAULT_KINDS");
    expect(retry).not.toMatch(/\.includes\(["']rate/);
    expect(retry).not.toMatch(/\.includes\(["']overload/);
    expect(retry).not.toMatch(/\.includes\(["']quota/);
  });

  it("keeps the legacy Session bridge isolated and its diagnostic payload fixed", () => {
    const adapter = source("legacy-session-provider-fault.ts");
    expect(adapter).toContain("if (Object.hasOwn(session, \"providerFault\")) return undefined");
    expect(adapter).not.toMatch(/session\s+as/);
    expect(adapter).not.toMatch(/session\.error\b/);
    expect(adapter).toContain("legacy_provider_fault_fallback kind=${fault.kind} source=session");
    expect(adapter).not.toContain("message=${");
  });
});
