/**
 * WS4 — empty/whitespace `idempotencyKey` is a FAIL-FAST throw (it used to
 * silently disable dedup: `?? generate()` kept `''`, then a downstream truthy
 * header-drop shipped no `Idempotency-Key`). Omitted keys auto-generate.
 */
import { describe, expect, it } from "vitest";
import { Aex, RunConfigValidationError } from "../../src/index.js";

function makeClient(): { client: Aex; keys: (string | undefined)[] } {
  const keys: (string | undefined)[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (url.endsWith("/api/sessions") && (init?.method ?? "GET") === "POST") {
      const headers = init?.headers instanceof Headers ? init.headers : new Headers(init?.headers as HeadersInit);
      keys.push(headers.get("idempotency-key") ?? undefined);
      return new Response(JSON.stringify({ session: { id: "run-1", status: "idle", turnSeq: 0 } }), {
        status: 201,
        headers: { "content-type": "application/json" }
      });
    }
    throw new Error(`no responder for ${url}`);
  };
  return { client: new Aex({ apiKey: "tk", baseUrl: "https://x", fetch }), keys };
}

describe("empty idempotencyKey fail-fast (WS4)", () => {
  const base = { model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } } as const;

  it("openSession/create throws synchronously on an empty key (no HTTP)", async () => {
    const { client, keys } = makeClient();
    await expect(client.openSession({ ...base, idempotencyKey: "" })).rejects.toBeInstanceOf(RunConfigValidationError);
    await expect(client.openSession({ ...base, idempotencyKey: "   " })).rejects.toBeInstanceOf(RunConfigValidationError);
    expect(keys).toEqual([]);
  });

  it("run/sessions.run throw synchronously on an empty key", async () => {
    const { client } = makeClient();
    await expect(client.run({ ...base, message: "hi", idempotencyKey: "" })).rejects.toBeInstanceOf(RunConfigValidationError);
    await expect(client.sessions.run({ ...base, message: "hi", idempotencyKey: "\t" })).rejects.toBeInstanceOf(
      RunConfigValidationError
    );
  });

  it("a valid key ships the Idempotency-Key header; an omitted key auto-generates one", async () => {
    const { client, keys } = makeClient();
    await client.openSession({ ...base, idempotencyKey: "my-key" });
    await client.openSession(base);
    expect(keys[0]).toBe("my-key");
    expect(keys[1]).toBeTruthy();
    expect(keys[1]).not.toBe("my-key");
  });
});
