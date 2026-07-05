/**
 * BLACKBOX — validate capability at the earliest seam, no fictional guarantees
 * (WS11 plane routing / P5, T15 tool entry).
 *
 * The findings this pins closed:
 *   WS11  a self-describing key whose plane can't be routed fails fast in the
 *         CONSTRUCTOR, ZERO-network, with a typed error — not a bare 401 after a
 *         full round-trip.
 *   T15   a tool authored with a non-JS entry is rejected at AUTHORING time — the
 *         validation happens where the developer is, not mid-run in the container.
 */
import { describe, expect, it } from "vitest";
import { Aex, CredentialValidationError, Tool } from "../../../src/index.js";
import { formatApiKey } from "@aexhq/contracts";

const WORKSPACE_ID = "0f9a1b2c-3d4e-5f60-7182-93a4b5c6d7e8";
const SECRET = "deadbeefcafef00dfeedface00c0ffee11223344556677";

describe("blackbox: plane routing guard (constructor, zero-network)", () => {
  it("throws a typed CredentialValidationError for a dev key with no baseUrl — before any fetch", () => {
    const devKey = formatApiKey({ plane: "dev", region: "eu-west-2", workspaceId: WORKSPACE_ID, secret: SECRET });
    let fetchCalls = 0;
    const spyFetch: typeof globalThis.fetch = async (...args) => {
      fetchCalls += 1;
      void args;
      return new Response("{}");
    };

    // The dev plane has no default host, so a self-describing dev key REQUIRES an
    // explicit baseUrl — the constructor fails fast rather than deferring to a 401.
    expect(() => new Aex({ apiKey: devKey, fetch: spyFetch })).toThrow(CredentialValidationError);
    // Zero-network: nothing was requested during the failed construction.
    expect(fetchCalls).toBe(0);
  });

  it("constructs fine when the same dev key is paired with an explicit baseUrl", () => {
    const devKey = formatApiKey({ plane: "dev", region: "eu-west-2", workspaceId: WORKSPACE_ID, secret: SECRET });
    expect(() => new Aex({ apiKey: devKey, baseUrl: "https://dev.aex.test", fetch: async () => new Response("{}") })).not.toThrow();
  });
});

describe("blackbox: tool entry validated at authoring", () => {
  it("rejects a non-JS tool entry at authoring time (not mid-run)", async () => {
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
