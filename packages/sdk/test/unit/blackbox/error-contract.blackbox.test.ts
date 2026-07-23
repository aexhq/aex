/**
 * BLACKBOX — the error contract (WS4 T12·T13).
 *
 * A customer must be able to BRANCH on why a call failed. The findings this pins:
 *   T13  a non-2xx wire failure surfaces as a TYPED subclass carrying a stable
 *        `apiCode` — not one opaque collapsed `API_ERROR`. `isIdempotencyConflict`
 *        / `isAuthError` / `isInsufficientScope` / `isNotFound` narrow it.
 *   T12  an empty / whitespace `idempotencyKey` is REJECTED before any request
 *        (it must never silently ship a non-idempotent, potentially double-billed
 *        run); a real key is forwarded as the `Idempotency-Key` header.
 *   T13  a client-side config error is a typed `SessionConfigValidationError`.
 */
import { describe, expect, it } from "bun:test";
import {
  SessionConfigValidationError,
  type SessionStartOptions,
  isAuthError,
  isIdempotencyConflict,
  isInsufficientScope,
  isNotFound
} from "../../../src/index.js";
import { FakePlatform } from "./fake-platform.js";

const SESSION: SessionStartOptions = { model: "claude-haiku-4-5", message: "go", apiKeys: { anthropic: "sk-ant" } };

describe("blackbox: typed error dispatch", () => {
  it("a 409 idempotency_conflict surfaces as a narrowable typed error with a stable apiCode", async () => {
    const platform = new FakePlatform();
    platform.scriptError({ pathIncludes: "/api/sessions", status: 409, code: "idempotency_conflict" });

    const err = await platform.start(SESSION).then(
      () => undefined,
      (e) => e as unknown
    );
    expect(err).toBeDefined();
    expect(isIdempotencyConflict(err)).toBe(true);
    // The stable, branchable code is carried as a typed apiCode — the WS4 core.
    expect((err as { apiCode?: string }).apiCode).toBe("idempotency_conflict");
    expect((err as Error).message.length).toBeGreaterThan(0);
  });

  it("a 403 insufficient_scope narrows as both an auth error and an insufficient-scope error", async () => {
    const platform = new FakePlatform();
    platform.scriptError({ pathIncludes: "/api/sessions", status: 403, code: "insufficient_scope" });

    const err = await platform.start(SESSION).then(
      () => undefined,
      (e) => e as unknown
    );
    expect(isAuthError(err)).toBe(true);
    expect(isInsufficientScope(err)).toBe(true);
  });

  it("a 404 not_found narrows as a not-found error", async () => {
    const platform = new FakePlatform();
    platform.scriptError({ pathIncludes: "/api/sessions", status: 404, code: "not_found" });

    const err = await platform.start(SESSION).then(
      () => undefined,
      (e) => e as unknown
    );
    expect(isNotFound(err)).toBe(true);
  });

  it("a 409 checkpoint_not_available remains a branchable state conflict", async () => {
    const platform = new FakePlatform();
    platform.scriptError({ pathIncludes: "/api/sessions", status: 409, code: "checkpoint_not_available" });

    const err = await platform.start(SESSION).then(
      () => undefined,
      (e) => e as unknown
    );
    expect(isIdempotencyConflict(err)).toBe(false);
    expect((err as { status?: number }).status).toBe(409);
    expect((err as { apiCode?: string }).apiCode).toBe("checkpoint_not_available");
  });
});

describe("blackbox: idempotency key is a real safety property", () => {
  it("rejects an empty idempotencyKey BEFORE any request (no silent non-idempotent run)", async () => {
    const platform = new FakePlatform();
    await expect(platform.start({ ...SESSION, idempotencyKey: "" })).rejects.toBeInstanceOf(SessionConfigValidationError);
    // Fail-closed: nothing was put on the wire.
    expect(platform.requestLog.length).toBe(0);
  });

  it("rejects a whitespace-only idempotencyKey", async () => {
    const platform = new FakePlatform();
    await expect(platform.start({ ...SESSION, idempotencyKey: "   " })).rejects.toBeInstanceOf(SessionConfigValidationError);
  });

  it("forwards a real idempotencyKey as the Idempotency-Key header on the create", async () => {
    const platform = new FakePlatform();
    await platform.start({ ...SESSION, idempotencyKey: "my-stable-key" }, { text: "ok", costUsd: 0.01 });
    const create = platform.requestLog.find((r) => r.method === "POST" && r.path.endsWith("/api/sessions"));
    expect(create?.idempotencyKey).toBe("my-stable-key");
  });
});
