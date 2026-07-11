/** `GET /api/whoami` has one canonical identity and admission-limits envelope. */
import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/http.js";
import { whoami } from "../src/operations.js";
import type { WhoAmI } from "../src/runtime-types.js";

function clientReturning(body: unknown): HttpClient {
  return new HttpClient({
    baseUrl: "https://api.example.test",
    apiKey: "test-token",
    fetch: async () =>
      new Response(JSON.stringify(body), {
        status: 200,
        headers: { "content-type": "application/json" }
      })
  });
}

const LIMITS: WhoAmI["limits"] = {
  maxConcurrentSessions: 50,
  submitRatePerMinute: 120,
  spendCapUsd: 250,
  monthSpendUsd: 12.5,
  balanceUsd: 100,
  balanceGraceFloorUsd: 0,
  balanceGateActive: true,
  paymentMethodStatus: "none",
  planKey: "free",
  accountType: "standard",
  subscriptionStatus: "none",
  subscriptionGate: "ok"
};

describe("whoami limits typing", () => {
  it("parses the canonical whoami response", async () => {
    const result = await whoami(
      clientReturning({
        ok: true,
        principalType: "api_key",
        workspaceId: "ws_1",
        scopes: ["sessions:read"],
        limits: LIMITS
      })
    );
    expect(result.workspaceId).toBe("ws_1");
    expect(result.limits).toEqual(LIMITS);
    // The typed field narrows without casts.
    expect(result.limits.maxConcurrentSessions).toBe(50);
    expect(result.limits.submitRatePerMinute).toBe(120);
    expect(result.limits.spendCapUsd).toBe(250);
    expect(result.limits.balanceGraceFloorUsd).toBe(0);
    expect(result.limits.paymentMethodStatus).toBe("none");
  });

  it("tolerates additive server metadata while projecting the known contract", async () => {
    const result = await whoami(
      clientReturning({
        ok: true,
        principalType: "api_key",
        workspaceId: "ws_1",
        scopes: ["sessions:read"],
        serverRevision: "2026-07-11",
        limits: { ...LIMITS, futureBurstWindow: 30 }
      })
    );

    expect(result).toEqual({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_1",
      scopes: ["sessions:read"],
      limits: LIMITS
    });
  });

  it.each([
    { ok: true, principalType: "api_key", workspaceId: "ws_1", scopes: [] },
    { ok: true, principalType: "api_key", workspaceId: "ws_1", scopes: [], caps: {}, limits: LIMITS },
    { ok: true, principalType: "api_key", workspaceId: "ws_1", scopes: [], tokenId: "legacy", limits: LIMITS },
    { ok: true, principalType: "api_key", workspaceId: "ws_1", scopes: [], tokenName: "legacy", limits: LIMITS },
    { ok: true, workspaceId: "ws_1", scopes: [], limits: LIMITS }
  ])("rejects incomplete or legacy identity envelopes", async (body) => {
    await expect(whoami(clientReturning(body))).rejects.toThrow();
  });

  it("rejects malformed known limit fields even alongside additive metadata", async () => {
    await expect(whoami(clientReturning({
      ok: true,
      principalType: "api_key",
      workspaceId: "ws_1",
      scopes: [],
      limits: { ...LIMITS, maxConcurrentSessions: "50", futureBurstWindow: 30 }
    }))).rejects.toThrow(/maxConcurrentSessions/);
  });
});
