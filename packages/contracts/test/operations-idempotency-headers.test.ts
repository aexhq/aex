/**
 * WS4: empty/whitespace idempotency keys FAIL FAST rather than silently
 * disabling dedup. `resolveIdempotencyKey` throws; the private
 * `idempotencyHeaders` (exercised via `createSession`) fails closed before any
 * fetch is issued.
 */
import { describe, expect, it } from "bun:test";
import {
  HttpClient,
  SessionConfigValidationError,
  type SessionCreateRequest
} from "../src/index.js";
import { isId } from "../src/ids.js";
import { operations } from "../src/internal.js";

const createRequest: SessionCreateRequest = {
  submission: {
    model: "deepseek/deepseek-v4-flash",
    assets: { files: [], skills: [], tools: [], instructions: [] },
    builtinTools: "default",
    mcpServers: []
  },
  secrets: {}
};

const http = new HttpClient({
  apiKey: "t",
  baseUrl: "https://api.test",
  fetch: async () => {
    throw new Error("fetch must not be called when the idempotency key is invalid");
  }
});

describe("idempotency key fail-closed (WS4)", () => {
  it("resolveIdempotencyKey rejects empty and whitespace-only keys", () => {
    expect(() => operations.resolveIdempotencyKey("")).toThrow(SessionConfigValidationError);
    expect(() => operations.resolveIdempotencyKey("   ")).toThrow(SessionConfigValidationError);
  });

  it("resolveIdempotencyKey returns a real key verbatim and generates one when absent", () => {
    expect(operations.resolveIdempotencyKey("k-123")).toBe("k-123");
    expect(isId("idempotency", operations.resolveIdempotencyKey())).toBe(true);
  });

  it("matches the hosted 255-character idempotency-key limit", () => {
    expect(operations.resolveIdempotencyKey("k".repeat(255))).toBe("k".repeat(255));
    expect(() => operations.resolveIdempotencyKey("k".repeat(256))).toThrow(SessionConfigValidationError);
  });

  /**
   * `deriveMessageIdempotencyKey` owns both branches of the first-message
   * identity. The SDK can only ever hand it a freshly minted `idem_<32 hex>`
   * (37 characters), so the long-key DIGEST branch is unreachable from there —
   * it is pinned here, at the layer that still accepts an arbitrary key.
   */
  it("derives a readable message key for short keys and a digest for long ones", () => {
    expect(operations.deriveMessageIdempotencyKey("k-123")).toBe("k-123:message");

    const longKey = "k".repeat(255);
    const derived = operations.deriveMessageIdempotencyKey(longKey);
    expect(derived).toMatch(/^aex-message-sha256-[a-f0-9]{64}$/);
    expect(derived.length).toBeLessThanOrEqual(255);
    // Deterministic: the same create key always yields the same message key.
    expect(operations.deriveMessageIdempotencyKey(longKey)).toBe(derived);
  });

  it("idempotencyHeaders fails closed on an empty create key before any fetch", async () => {
    await expect(
      operations.createSession(http, createRequest, { idempotencyKey: "" })
    ).rejects.toBeInstanceOf(SessionConfigValidationError);
  });

  it("a valid create idempotency key ships the Idempotency-Key header", async () => {
    let seenHeader: string | null = null;
    const capture = new HttpClient({
      apiKey: "t",
      baseUrl: "https://api.test",
      fetch: async (_input, init) => {
        seenHeader = new Headers(init?.headers).get("Idempotency-Key");
        return new Response(JSON.stringify({ session: { id: "sess_1", status: "idle", acceptsMessages: true } }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });
    await operations.createSession(capture, createRequest, { idempotencyKey: "key-abc" });
    expect<string | null>(seenHeader).toBe("key-abc");
  });
});

describe("lifecycle controls are not advertised as idempotent", () => {
  const controls = [
    ["suspend", operations.suspendSession],
    ["cancel", operations.cancelSession],
    ["resume", operations.resumeSession],
    ["request approval", operations.requestApproval],
    ["approve", operations.approveSession],
    ["deny", operations.denySession],
    ["delete", operations.deleteSession]
  ] as const;

  it.each(controls)("%s sends no Idempotency-Key", async (_name, operation) => {
    let seenHeader: string | null = null;
    const capture = new HttpClient({
      apiKey: "t",
      baseUrl: "https://api.test",
      fetch: async (_input, init) => {
        seenHeader = new Headers(init?.headers).get("Idempotency-Key");
        return new Response(
          JSON.stringify({
            session: { id: "sess_1", status: "idle", acceptsMessages: true },
            // DELETE alone answers with the footprint-cleanup counters beside
            // the session; the six state-change routes answer `{ session }` and
            // nothing else. One body serves both cases here because the extra
            // keys are ignored by the state-change reads and REQUIRED by the
            // delete read, which no longer treats a bodyless 204 as possible.
            purgedSessionFileObjects: 0,
            cleanupComplete: true
          }),
          { status: 200, headers: { "content-type": "application/json" } }
        );
      }
    });

    await operation(capture, "sess_1");
    expect(seenHeader).toBeNull();
  });
});

describe("billing mutation identities", () => {
  it.each([
    ["topup checkout", operations.createBillingTopupCheckout, { amountUsd: 25 }],
    ["portal", operations.createBillingPortal, { returnUrl: "https://aex.dev/billing" }]
  ] as const)("%s sends identity only as a header", async (_name, operation, request) => {
    let seenHeader: string | null = null;
    let seenBody: unknown;
    const capture = new HttpClient({
      apiKey: "t",
      baseUrl: "https://api.test",
      fetch: async (_input, init) => {
        seenHeader = new Headers(init?.headers).get("Idempotency-Key");
        seenBody = JSON.parse(String(init?.body));
        return new Response(JSON.stringify({ url: "https://billing.test/session" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });

    await operation(capture, request as never, { idempotencyKey: "billing-key" });

    expect<string | null>(seenHeader).toBe("billing-key");
    expect(seenBody).toEqual(request);
    expect(seenBody).not.toHaveProperty("idempotencyKey");
  });

  it("generates a key before dispatch and rejects legacy or oversized keys locally", async () => {
    let seenHeader: string | null = null;
    const capture = new HttpClient({
      apiKey: "t",
      baseUrl: "https://api.test",
      fetch: async (_input, init) => {
        seenHeader = new Headers(init?.headers).get("Idempotency-Key");
        return new Response(JSON.stringify({ url: "https://billing.test/session" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });

    await operations.createBillingTopupCheckout(capture, { amountUsd: 25 });
    expect(isId("idempotency", seenHeader)).toBe(true);
    await expect(
      operations.createBillingTopupCheckout(capture, { amountUsd: 25, idempotencyKey: "legacy" } as never)
    ).rejects.toBeInstanceOf(SessionConfigValidationError);
    await expect(
      operations.createBillingPortal(capture, {}, { idempotencyKey: "x".repeat(256) })
    ).rejects.toBeInstanceOf(SessionConfigValidationError);
  });

  it("sends NO idempotency header for the auto-topup PATCH but still refuses a body key", async () => {
    let seenHeader: string | null = null;
    let seenMethod: string | undefined;
    const capture = new HttpClient({
      apiKey: "t",
      baseUrl: "https://api.test",
      fetch: async (_input, init) => {
        seenHeader = new Headers(init?.headers).get("Idempotency-Key");
        seenMethod = init?.method;
        return new Response(
          JSON.stringify({
            autoTopup: { enabled: true, thresholdUsd: 5, amountUsd: 20, minimumAmountUsd: 10, maxPerDay: 4 }
          }),
          { status: 200, headers: { "content-type": "application/json" } }
        );
      }
    });

    // A whole-state PATCH replays to the same row, so there is nothing for a key
    // to deduplicate and sending one would imply a guarantee the route has not made.
    const updated = await operations.updateBillingAutoTopup(capture, { enabled: true });
    expect(seenMethod).toBe("PATCH");
    expect<string | null>(seenHeader).toBeNull();
    expect(updated.autoTopup.enabled).toBe(true);

    await expect(
      operations.updateBillingAutoTopup(capture, { enabled: true, idempotencyKey: "legacy" } as never)
    ).rejects.toBeInstanceOf(SessionConfigValidationError);
  });
});
