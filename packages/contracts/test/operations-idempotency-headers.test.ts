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
          JSON.stringify({ session: { id: "sess_1", status: "idle", acceptsMessages: true } }),
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
    ["checkout", operations.createBillingCheckout, { planKey: "pro" }],
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

    await operations.createBillingCheckout(capture, { planKey: "team" });
    expect(isId("idempotency", seenHeader)).toBe(true);
    await expect(
      operations.createBillingCheckout(capture, { planKey: "pro", idempotencyKey: "legacy" } as never)
    ).rejects.toBeInstanceOf(SessionConfigValidationError);
    await expect(
      operations.createBillingPortal(capture, {}, { idempotencyKey: "x".repeat(256) })
    ).rejects.toBeInstanceOf(SessionConfigValidationError);
  });
});
