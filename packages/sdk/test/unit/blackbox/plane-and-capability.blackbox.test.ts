/**
 * BLACKBOX — validate capability at the earliest seam, no fictional guarantees
 * (WS11 plane routing / P5, T15 tool entry).
 *
 * The findings this pins closed:
 *   WS11  self-describing keys route to their canonical plane host with
 *         ZERO-network constructor behavior and no late wrong-plane 401.
 *   T15   a tool authored with a non-JS entry is rejected at AUTHORING time — the
 *         validation happens where the developer is, not mid-session in the container.
 */
import { describe, expect, it } from "vitest";
import { Aex, Tool } from "../../../src/index.js";
import { formatApiKey } from "@aexhq/contracts";

const WORKSPACE_ID = "0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8";
const SECRET = "deadbeefcafef00dfeedface00c0ffee11223344556677";
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
    balanceGateActive: true,
    paymentMethodStatus: "none",
    planKey: "free",
    accountType: "standard",
    subscriptionStatus: "none",
    subscriptionGate: "ok"
  }
};

describe("blackbox: plane routing guard (constructor, zero-network)", () => {
  it("routes a dev key with no baseUrl to dev-api.aex.dev", async () => {
    const devKey = formatApiKey({ plane: "dev", region: "eu-west-2", workspaceId: WORKSPACE_ID, secret: SECRET });
    const seen: string[] = [];
    const spyFetch: typeof globalThis.fetch = async (...args) => {
      const [input] = args;
      seen.push(typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url);
      return new Response(JSON.stringify(whoami), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    };

    const client = new Aex({ apiKey: devKey, fetch: spyFetch });
    expect(seen).toEqual([]);
    await client.whoami();
    expect(seen[0]).toContain("https://dev-api.aex.dev");
  });

  it("constructs fine when the same dev key is paired with an explicit baseUrl", () => {
    const devKey = formatApiKey({ plane: "dev", region: "eu-west-2", workspaceId: WORKSPACE_ID, secret: SECRET });
    expect(() => new Aex({ apiKey: devKey, baseUrl: "https://dev.aex.test", fetch: async () => new Response("{}") })).not.toThrow();
  });
});

describe("blackbox: tool entry validated at authoring", () => {
  it("rejects a non-JS tool entry at authoring time (not mid-session)", async () => {
    await expect(
      Tool.fromFiles({
        name: "shell-tool",
        description: "runs a shell script",
        inputSchema: { type: "object" },
        entry: "run.sh",
        files: { "run.sh": "#!/bin/sh\necho hi" }
      })
    ).rejects.toThrow(/JS module/);
  });

  it("accepts a valid JS module entry", async () => {
    const tool = await Tool.fromFiles({
      name: "js-tool",
      description: "a real JS tool",
      inputSchema: { type: "object" },
      entry: "index.mjs",
      files: { "index.mjs": "export default async function () { return { ok: true }; }" }
    });
    // The accepted entry was normalized onto the ref (an SDK-computed value, not a constant).
    expect(tool.ref.entry).toBe("index.mjs");
  });
});
