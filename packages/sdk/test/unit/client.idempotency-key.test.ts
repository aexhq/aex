/**
 * WS4 — empty/whitespace `idempotencyKey` is a FAIL-FAST throw (it used to
 * silently disable dedup: `?? generate()` kept `''`, then a downstream truthy
 * header-drop shipped no `Idempotency-Key`). Omitted keys auto-generate.
 */
import { describe, expect, it } from "vitest";
import { Aex, SessionConfigValidationError } from "../../src/index.js";

function makeClient(): { client: Aex; keys: (string | undefined)[] } {
  const keys: (string | undefined)[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (url.endsWith("/api/sessions") && (init?.method ?? "GET") === "POST") {
      const headers = init?.headers instanceof Headers ? init.headers : new Headers(init?.headers as HeadersInit);
      keys.push(headers.get("idempotency-key") ?? undefined);
      return new Response(JSON.stringify({ session: { id: "session-1", status: "idle", acceptsMessages: true } }), {
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

  it("sessions.create throws synchronously on an empty key (no HTTP)", async () => {
    const { client, keys } = makeClient();
    await expect(client.sessions.create({ ...base, idempotencyKey: "" })).rejects.toBeInstanceOf(SessionConfigValidationError);
    await expect(client.sessions.create({ ...base, idempotencyKey: "   " })).rejects.toBeInstanceOf(SessionConfigValidationError);
    expect(keys).toEqual([]);
  });

  it("Aex.start throws synchronously on an empty key", async () => {
    const { client } = makeClient();
    await expect(client.start({ ...base, message: "hi", idempotencyKey: "" })).rejects.toBeInstanceOf(SessionConfigValidationError);
    await expect(client.start({ ...base, message: "hi", idempotencyKey: "\t" })).rejects.toBeInstanceOf(
      SessionConfigValidationError
    );
  });

  it("a valid key ships the Idempotency-Key header; an omitted key auto-generates one", async () => {
    const { client, keys } = makeClient();
    await client.sessions.create({ ...base, idempotencyKey: "my-key" });
    await client.sessions.create(base);
    expect(keys[0]).toBe("my-key");
    expect(keys[1]).toBeTruthy();
    expect(keys[1]).not.toBe("my-key");
  });

  it("accepts 255 characters and rejects 256 before HTTP", async () => {
    const { client, keys } = makeClient();
    await client.sessions.create({ ...base, idempotencyKey: "k".repeat(255) });
    await expect(client.sessions.create({ ...base, idempotencyKey: "k".repeat(256) }))
      .rejects.toBeInstanceOf(SessionConfigValidationError);
    expect(keys).toEqual(["k".repeat(255)]);
  });

  it("rejects an oversized Aex.start message key before creating a session", async () => {
    const { client, keys } = makeClient();
    await expect(client.start({
      ...base,
      message: "hello",
      idempotencyKey: "create-key",
      messageIdempotencyKey: "m".repeat(256)
    })).rejects.toBeInstanceOf(SessionConfigValidationError);
    expect(keys).toEqual([]);
  });
});
