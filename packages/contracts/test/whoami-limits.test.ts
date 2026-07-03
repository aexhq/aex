/**
 * `GET /whoami` — the ADDITIVE `limits` object.
 *
 * Current platform deployments return an effective per-workspace `limits`
 * object on `whoami` (concurrency cap, submit rate, spend cap, balance and
 * grace floor, payment-method status); older deployments omit it. The public
 * `WhoAmI` type carries it as an optional typed field, so both shapes must
 * parse through `operations.whoami` unchanged.
 */
import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/http.js";
import { whoami } from "../src/operations.js";
import type { WhoAmI } from "../src/runtime-types.js";

function clientReturning(body: unknown): HttpClient {
  return new HttpClient({
    baseUrl: "https://api.example.test",
    apiToken: "test-token",
    fetch: async () =>
      new Response(JSON.stringify(body), {
        status: 200,
        headers: { "content-type": "application/json" }
      })
  });
}

const LIMITS: NonNullable<WhoAmI["limits"]> = {
  maxConcurrentRuns: 50,
  submitRatePerMinute: 120,
  spendCapUsd: 250,
  monthSpendUsd: 12.5,
  balanceUsd: 100,
  balanceGraceFloorUsd: 0,
  paymentMethodStatus: "none"
};

describe("whoami limits typing", () => {
  it("parses a whoami response that carries the limits object", async () => {
    const result = await whoami(
      clientReturning({ ok: true, workspaceId: "ws_1", scopes: ["runs:read"], limits: LIMITS })
    );
    expect(result.workspaceId).toBe("ws_1");
    expect(result.limits).toEqual(LIMITS);
    // The typed field narrows without casts.
    expect(result.limits?.maxConcurrentRuns).toBe(50);
    expect(result.limits?.submitRatePerMinute).toBe(120);
    expect(result.limits?.spendCapUsd).toBe(250);
    expect(result.limits?.balanceGraceFloorUsd).toBe(0);
    expect(result.limits?.paymentMethodStatus).toBe("none");
  });

  it("parses a whoami response without limits (older deployments)", async () => {
    const result = await whoami(clientReturning({ ok: true, workspaceId: "ws_1", scopes: [] }));
    expect(result.workspaceId).toBe("ws_1");
    expect(result.limits).toBeUndefined();
  });

  it("accepts an 'active' payment-method status with a folded overdraft floor", async () => {
    const result = await whoami(
      clientReturning({
        ok: true,
        workspaceId: "ws_2",
        limits: { ...LIMITS, paymentMethodStatus: "active", balanceGraceFloorUsd: -20 }
      })
    );
    expect(result.limits?.paymentMethodStatus).toBe("active");
    expect(result.limits?.balanceGraceFloorUsd).toBe(-20);
  });
});
