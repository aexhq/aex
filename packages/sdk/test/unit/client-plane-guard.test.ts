/**
 * WS11 — the constructor parses the self-describing API key and routes by plane
 * ZERO-network: a plane/baseUrl mismatch throws before any request; a prd key
 * with no baseUrl auto-derives `api.aex.dev`; a dev key with no baseUrl
 * auto-derives `dev-api.aex.dev`.
 */
import { describe, expect, it, vi } from "vitest";
import { formatApiKey, PLANE_BASE_URLS } from "@aexhq/contracts";
import { Aex, CredentialValidationError } from "../../src/index.js";

const devKey = formatApiKey({ plane: "dev", region: "eu-west-2", workspaceId: "ws123", secret: "s3cr3tvalue" });
const prdKey = formatApiKey({ plane: "prd", region: "eu-west-2", workspaceId: "ws123", secret: "s3cr3tvalue" });
const whoami = {
  ok: true,
  principalType: "api_key",
  workspaceId: "ws123",
  scopes: [],
  limits: {
    maxConcurrentSessions: 1,
    submitRatePerMinute: 0,
    spendCapUsd: 0,
    monthSpendUsd: 0,
    balanceUsd: 0,
    balanceGraceFloorUsd: 0,
    balanceGateActive: true,
    paymentMethodStatus: "none",
    planKey: "free",
    accountType: "standard",
    subscriptionStatus: "none",
    subscriptionGate: "ok"
  }
};

describe("constructor plane guard (WS11)", () => {
  it("throws on a dev key pointed at the prd host, making ZERO fetch calls", () => {
    const fetch = vi.fn(async () => new Response("{}"));
    expect(() => new Aex(devKey, { baseUrl: PLANE_BASE_URLS.prd, fetch: fetch as unknown as typeof globalThis.fetch })).toThrow(
      CredentialValidationError
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it("throws on a prd key pointed at the dev host, making ZERO fetch calls", () => {
    const fetch = vi.fn(async () => new Response("{}"));
    expect(() => new Aex(prdKey, { baseUrl: PLANE_BASE_URLS.dev, fetch: fetch as unknown as typeof globalThis.fetch })).toThrow(
      CredentialValidationError
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it("a prd key with no baseUrl auto-routes to api.aex.dev", async () => {
    const seen: string[] = [];
    const fetch: typeof globalThis.fetch = async (input) => {
      seen.push(typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url);
      return new Response(JSON.stringify(whoami), { status: 200, headers: { "content-type": "application/json" } });
    };
    const client = new Aex(prdKey, { fetch });
    await client.whoami();
    expect(seen[0]).toContain(PLANE_BASE_URLS.prd);
  });

  it("a dev key with no baseUrl auto-routes to dev-api.aex.dev", async () => {
    const seen: string[] = [];
    const fetch: typeof globalThis.fetch = async (input) => {
      seen.push(typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url);
      return new Response(JSON.stringify(whoami), { status: 200, headers: { "content-type": "application/json" } });
    };
    const client = new Aex(devKey, { fetch });
    await client.whoami();
    expect(seen[0]).toContain(PLANE_BASE_URLS.dev);
  });

  it("a dev key with an explicit non-prd baseUrl is accepted", () => {
    expect(() => new Aex(devKey, { baseUrl: "https://dev-plane.example.test" })).not.toThrow();
  });

  it("an opaque/legacy key skips plane routing (no throw)", () => {
    expect(() => new Aex("opaque-legacy-token", { baseUrl: "https://x" })).not.toThrow();
  });
});
