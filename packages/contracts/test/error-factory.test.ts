/**
 * WS4 class-killer: one error factory (wire → typed subclass) backed by the
 * stable-code SSoT. Maps 409/403/404/401/429 to the right subclass with
 * apiCode + requestId + cause, and an exhaustive switch over AexApiErrorCode.
 */
import { describe, expect, it } from "vitest";
import {
  apiErrorFromResponse,
  apiErrorKindForCode,
  AEX_API_ERROR_CODES,
  AexApiError,
  AexAuthError,
  AexIdempotencyConflictError,
  AexNotFoundError,
  AexRateLimitError,
  isAuthError,
  isInsufficientScope,
  isIdempotencyConflict,
  isNotFound,
  isRateLimited,
  type AexApiErrorCode
} from "../src/index.js";

describe("apiErrorFromResponse (WS4)", () => {
  it("maps 409 idempotency_conflict → AexIdempotencyConflictError", () => {
    const err = apiErrorFromResponse({ status: 409, body: { error: "idempotency_conflict", requestId: "req-1" } });
    expect(err).toBeInstanceOf(AexIdempotencyConflictError);
    expect(err).toBeInstanceOf(AexApiError);
    expect(err.apiCode).toBe("idempotency_conflict");
    expect(err.requestId).toBe("req-1");
    expect(isIdempotencyConflict(err)).toBe(true);
    expect(err.message.length).toBeGreaterThan(0);
  });

  it("maps 403 insufficient_scope → AexAuthError carrying requiredScope", () => {
    const err = apiErrorFromResponse({
      status: 403,
      body: { error: "insufficient_scope", requiredScope: "billing:read", requestId: "req-2" }
    });
    expect(err).toBeInstanceOf(AexAuthError);
    expect(err.apiCode).toBe("insufficient_scope");
    expect((err as AexAuthError).requiredScope).toBe("billing:read");
    expect(isAuthError(err)).toBe(true);
    expect(isInsufficientScope(err)).toBe(true);
  });

  it("maps 404 → AexNotFoundError", () => {
    const err = apiErrorFromResponse({ status: 404, body: { error: "not_found" } });
    expect(err).toBeInstanceOf(AexNotFoundError);
    expect(isNotFound(err)).toBe(true);
    expect(err.apiCode).toBe("not_found");
  });

  it("maps 401 token_invalid → AexAuthError", () => {
    const err = apiErrorFromResponse({ status: 401, body: { error: "token_invalid" } });
    expect(err).toBeInstanceOf(AexAuthError);
    expect(err.apiCode).toBe("token_invalid");
  });

  it("maps 429 rate_limited → AexRateLimitError with retryAfterMs", () => {
    const err = apiErrorFromResponse({ status: 429, body: { error: "rate_limited", retryAfter: 2 } });
    expect(err).toBeInstanceOf(AexRateLimitError);
    expect(isRateLimited(err)).toBe(true);
    expect((err as AexRateLimitError).retryAfterMs).toBe(2000);
  });

  it("uses the precomputed message and threads cause", () => {
    const cause = new Error("boom");
    const err = apiErrorFromResponse({
      status: 500,
      body: { error: "internal_error" },
      message: "internal_error: kaput",
      cause
    });
    expect(err.message).toBe("internal_error: kaput");
    expect(err.cause).toBe(cause);
    expect(err.apiCode).toBe("internal_error");
  });

  it("falls back to status-based dispatch for an unknown code (apiCode undefined)", () => {
    const err = apiErrorFromResponse({ status: 404, body: { error: "totally_unknown_code" } });
    expect(err).toBeInstanceOf(AexNotFoundError);
    expect(err.apiCode).toBeUndefined();
  });

  it("every stable code maps to a kind with no throw", () => {
    for (const code of AEX_API_ERROR_CODES) {
      expect(typeof apiErrorKindForCode(code)).toBe("string");
    }
  });

  it("[compile-time] exhaustive switch over AexApiErrorCode with no default", () => {
    const kindOf = (code: AexApiErrorCode): string => {
      switch (code) {
        case "unauthorized":
        case "forbidden":
        case "insufficient_scope":
        case "token_invalid":
        case "token_revoked":
        case "token_expired":
        case "malformed_token":
          return "auth";
        case "idempotency_conflict":
          return "idempotency";
        case "not_found":
          return "not_found";
        case "rate_limited":
        case "workspace_concurrency_exceeded":
        case "workspace_submit_rate_exceeded":
          return "rate_limit";
        case "session_busy":
        case "session_not_terminal":
        case "unknown_workspace":
        case "workspace_spend_cap_exceeded":
        case "insufficient_balance":
        case "upstream_error":
        case "internal_error":
          return "generic";
      }
    };
    expect(kindOf("not_found")).toBe("not_found");
  });
});
