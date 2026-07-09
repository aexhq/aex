/**
 * WS4: empty/whitespace idempotency keys FAIL FAST rather than silently
 * disabling dedup. `resolveIdempotencyKey` throws; the private
 * `idempotencyHeaders` (exercised via `suspendSession`) fails closed before any
 * fetch is issued.
 */
import { describe, expect, it } from "vitest";
import { HttpClient, SessionConfigValidationError, operations } from "../src/index.js";

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
    expect(operations.resolveIdempotencyKey().startsWith("aex-idem-")).toBe(true);
  });

  it("idempotencyHeaders fails closed on an empty key before any fetch (via suspendSession)", async () => {
    await expect(
      operations.suspendSession(http, "sess_1", { idempotencyKey: "" })
    ).rejects.toBeInstanceOf(SessionConfigValidationError);
  });

  it("a valid idempotency key ships the Idempotency-Key header", async () => {
    let seenHeader: string | null = null;
    const capture = new HttpClient({
      apiKey: "t",
      baseUrl: "https://api.test",
      fetch: async (_input, init) => {
        seenHeader = new Headers(init?.headers).get("Idempotency-Key");
        return new Response(JSON.stringify({ session: { id: "sess_1", status: "suspending" } }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
    });
    await operations.suspendSession(capture, "sess_1", { idempotencyKey: "key-abc" });
    expect(seenHeader).toBe("key-abc");
  });
});
