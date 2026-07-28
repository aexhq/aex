/**
 * The idempotency key is SDK-OWNED. Callers neither supply nor manage one.
 *
 * This file used to pin the caller-supplied key's validation (WS4 fail-fast on
 * an empty/whitespace key). That surface is gone: the SDK mints the key, so
 * there is no caller value left to validate. The key POLICY itself still lives
 * in `@aexhq/contracts` and is covered by
 * `packages/contracts/test/operations-idempotency-headers.test.ts`.
 *
 * What is asserted here instead:
 *   1. Every mutation still ships a well-formed `Idempotency-Key` — the wire
 *      contract is UNCHANGED, only its owner moved.
 *   2. Passing the retired option is REJECTED loudly rather than ignored
 *      silently, with guidance that says the SDK now handles it.
 *
 * Stability of that key across automatic retries — the property the whole
 * mechanism exists for — is pinned in `client.idempotency-retry-stability.test.ts`.
 */
import { describe, expect, it } from "bun:test";
import { idPattern, type FetchLike } from "@aexhq/contracts";
import { Aex, SessionConfigValidationError } from "../../src/index.js";

function makeClient(): { client: Aex; keys: (string | undefined)[] } {
  const keys: (string | undefined)[] = [];
  const fetch: FetchLike = async (input, init) => {
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

/** The minted identifier's canonical form, per `@aexhq/contracts` `newId`. */
const MINTED_KEY = idPattern("idempotency");

describe("the SDK owns the idempotency key", () => {
  const base = { model: "anthropic/claude-haiku-4-5" } as const;

  it("mints a well-formed key for every create without the caller doing anything", async () => {
    const { client, keys } = makeClient();
    await client.sessions.create(base);
    await client.sessions.create(base);

    expect(keys).toHaveLength(2);
    for (const key of keys) expect(key).toMatch(MINTED_KEY);
    // Two SEPARATE logical creates are two separate identities.
    expect(keys[0]).not.toBe(keys[1]);
  });

  it("rejects a caller-supplied idempotencyKey on sessions.create, before any HTTP", async () => {
    const { client, keys } = makeClient();
    await expect(
      client.sessions.create({ ...base, idempotencyKey: "my-key" } as never)
    ).rejects.toBeInstanceOf(SessionConfigValidationError);
    expect(keys).toEqual([]);
  });

  it("rejects a caller-supplied idempotencyKey on Aex.start", async () => {
    const { client, keys } = makeClient();
    await expect(
      client.start({ ...base, message: "hi", idempotencyKey: "my-key" } as never)
    ).rejects.toBeInstanceOf(SessionConfigValidationError);
    expect(keys).toEqual([]);
  });

  it("rejects the retired messageIdempotencyKey on Aex.start", async () => {
    const { client, keys } = makeClient();
    await expect(
      client.start({ ...base, message: "hi", messageIdempotencyKey: "m" } as never)
    ).rejects.toBeInstanceOf(SessionConfigValidationError);
    expect(keys).toEqual([]);
  });

  it("explains that the SDK now owns the key rather than just naming the field", async () => {
    const { client } = makeClient();
    const error: unknown = await client.sessions
      .create({ ...base, idempotencyKey: "my-key" } as never)
      .catch((err: unknown) => err);

    expect(error).toBeInstanceOf(SessionConfigValidationError);
    const failure = error as SessionConfigValidationError;
    expect(failure.details.field).toBe("idempotencyKey");
    expect(failure.message).toContain("the SDK now owns idempotency");
    // The guidance names the automatic retries, because that is the reason a
    // caller-managed key is no longer needed.
    expect(failure.message).toContain("automatic retries");
  });
});
