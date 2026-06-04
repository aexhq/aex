/**
 * The SDK fails early when the caller explicitly asks for a runtime selector
 * outside the public enum — a typed AexError thrown CLIENT-SIDE, before
 * any HTTP request.
 */
import { describe, expect, it } from "vitest";
import { AexClient } from "../../src/index.js";
import { AexError } from "@aexhq/contracts";

function recordingFetch(): { fetch: typeof fetch; calls: string[] } {
  const calls: string[] = [];
  const f: typeof fetch = async (input) => {
    calls.push(typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url);
    return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } });
  };
  return { fetch: f, calls };
}

describe("AexClient.submitRun — client-side runtime validation", () => {
  it("throws AexError(CREDENTIAL_INVALID) for managed-key mode without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new AexClient({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.submitRun({
        credentialMode: "managed",
        model: "claude-haiku-4-5",
        prompt: "hi"
      } as Parameters<AexClient["submitRun"]>[0])
    ).rejects.toMatchObject({ name: "AexError", code: "CREDENTIAL_INVALID" });

    expect(rec.calls).toHaveLength(0);
  });

  it("throws AexError(RUNTIME_UNSUPPORTED) for native, WITHOUT any HTTP call", async () => {
    const rec = recordingFetch();
    const client = new AexClient({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.submitRun({
        provider: "deepseek",
        runtime: "native",
        model: "deepseek-chat",
        prompt: "hi",
        secrets: { deepseek: { apiKey: "sk-x" } }
      } as unknown as Parameters<AexClient["submitRun"]>[0])
    ).rejects.toMatchObject({ name: "AexError", code: "RUNTIME_UNSUPPORTED" });

    // The rejection happened before the network: no request was made.
    expect(rec.calls).toHaveLength(0);
  });

  it("surfaces a 'native' hint in the message", async () => {
    const rec = recordingFetch();
    const client = new AexClient({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    let caught: unknown;
    try {
      await client.submitRun({
        provider: "deepseek",
        runtime: "native",
        model: "deepseek-chat",
        prompt: "hi",
        secrets: { deepseek: { apiKey: "sk-x" } }
      } as unknown as Parameters<AexClient["submitRun"]>[0]);
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(AexError);
    expect((caught as Error).message.toLowerCase()).toContain("native");
  });
});
