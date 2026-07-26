/**
 * WS11 — the constructor parses the self-describing API key and routes by plane
 * ZERO-network: a plane/baseUrl mismatch throws before any request; a prd key
 * with no baseUrl auto-derives `api.aex.dev`; a dev key with no baseUrl
 * auto-derives `dev-api.aex.dev`. A non-self-describing account PAT (`aexu_…`)
 * is NOT plane-routable, so with no baseUrl it defaults to the prd host.
 */
import { describe, expect, it, mock } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { AEX_DEFAULT_BASE_URL, formatApiKey, PLANE_BASE_URLS } from "@aexhq/contracts";
import { Aex, CredentialValidationError } from "../../src/index.js";

const WORKSPACE_ID = "wsp_00000000000000000000000000000001";
const devKey = formatApiKey({ plane: "dev", region: "eu-west-1", workspaceId: WORKSPACE_ID, secret: "s3cr3tvalue" });
const prdKey = formatApiKey({ plane: "prd", region: "eu-west-1", workspaceId: WORKSPACE_ID, secret: "s3cr3tvalue" });
const whoami = {
  ok: true,
  principalType: "api_key",
  workspaceId: WORKSPACE_ID,
  scopes: [],
  limits: {
    maxConcurrentSessions: 1,
    submitRatePerMinute: 0,
    spendCapUsd: 0,
    monthSpendUsd: 0,
    balanceUsd: 0,
    balanceGraceFloorUsd: 0,
    llmTokenAllowanceRemainingUsd: 2,
    creditGateActive: true,
    paymentMethodStatus: "none",
    admissionState: "free",
    autoTopupEnabled: false,
    accountType: "standard"
  }
};

describe("constructor plane guard (WS11)", () => {
  it("throws on a dev key pointed at the prd host, making ZERO fetch calls", () => {
    const fetch = mock(async () => new Response("{}"));
    expect(() => new Aex(devKey, { baseUrl: PLANE_BASE_URLS.prd, fetch })).toThrow(
      CredentialValidationError
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it("throws on a prd key pointed at the dev host, making ZERO fetch calls", () => {
    const fetch = mock(async () => new Response("{}"));
    expect(() => new Aex(prdKey, { baseUrl: PLANE_BASE_URLS.dev, fetch })).toThrow(
      CredentialValidationError
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it("a prd key with no baseUrl auto-routes to api.aex.dev", async () => {
    const seen: string[] = [];
    const fetch: FetchLike = async (input) => {
      seen.push(typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url);
      return new Response(JSON.stringify(whoami), { status: 200, headers: { "content-type": "application/json" } });
    };
    const client = new Aex(prdKey, { fetch });
    await client.whoami();
    expect(seen[0]).toContain(PLANE_BASE_URLS.prd);
  });

  it("a dev key with no baseUrl auto-routes to dev-api.aex.dev", async () => {
    const seen: string[] = [];
    const fetch: FetchLike = async (input) => {
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

  it("an account PAT (aexu_…) with no baseUrl defaults to the prd host api.aex.dev", async () => {
    // A PAT is not self-describing → resolveBaseUrlForKey returns undefined →
    // the HttpClient default (prd) applies. Pins the SDK-layer behavior that,
    // until now, was only exercised through the CLI's control-plane resolver.
    const seen: string[] = [];
    const fetch: FetchLike = async (input) => {
      seen.push(typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url);
      return new Response(JSON.stringify(whoami), { status: 200, headers: { "content-type": "application/json" } });
    };
    const client = new Aex({ apiKey: "aexu_pat_tokentokentokentoken", fetch });
    await client.whoami();
    expect(AEX_DEFAULT_BASE_URL).toBe(PLANE_BASE_URLS.prd);
    expect(seen[0]).toBe(`${AEX_DEFAULT_BASE_URL}/api/whoami`);
  });
});
