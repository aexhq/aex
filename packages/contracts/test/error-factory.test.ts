/**
 * WS4 class-killer: one error factory (wire → typed subclass) backed by the
 * stable-code SSoT. Maps 409 idempotency conflicts plus 403/404/401/429 to the
 * right subclass with apiCode + requestId + cause, and keeps unrelated 409s
 * generic through an exhaustive switch over AexApiErrorCode.
 */
import { describe, expect, it } from "bun:test";
import {
  apiErrorFromResponse,
  apiErrorKindForCode,
  AEX_API_ERROR_CODES,
  AEX_API_ERROR_MESSAGES,
  AEX_API_ERROR_REMEDIES,
  AexApiError,
  AexAuthError,
  AexIdempotencyConflictError,
  AexNotFoundError,
  AexRateLimitError,
  ContentDeletedError,
  isAuthError,
  isContentDeleted,
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

  it("maps 410 content_deleted → ContentDeletedError carrying sessionId/purgedAt/deletedBy (WS4)", () => {
    const body = {
      error: "content_deleted",
      sessionId: "sess-purged",
      purgedAt: "2026-07-17T00:00:00.000Z",
      deletedBy: "retention",
      requestId: "req-410"
    };
    const err = apiErrorFromResponse({ status: 410, body });
    expect(err).toBeInstanceOf(ContentDeletedError);
    expect(err).toBeInstanceOf(AexApiError);
    expect(isContentDeleted(err)).toBe(true);
    expect(err.apiCode).toBe("content_deleted");
    expect(err.status).toBe(410);
    expect(err.requestId).toBe("req-410");
    const deleted = err as ContentDeletedError;
    expect(deleted.sessionId).toBe("sess-purged");
    expect(deleted.purgedAt).toBe("2026-07-17T00:00:00.000Z");
    expect(deleted.deletedBy).toBe("retention");
  });

  it("tolerates a content_deleted body missing the optional purge fields", () => {
    const err = apiErrorFromResponse({ status: 410, body: { error: "content_deleted" } });
    expect(err).toBeInstanceOf(ContentDeletedError);
    const deleted = err as ContentDeletedError;
    expect(deleted.sessionId).toBeUndefined();
    expect(deleted.purgedAt).toBeUndefined();
    expect(deleted.deletedBy).toBeUndefined();
    // An unrecognized `deletedBy` never leaks through the closed union.
    const weird = apiErrorFromResponse({
      status: 410,
      body: { error: "content_deleted", deletedBy: "aliens" }
    }) as ContentDeletedError;
    expect(weird.deletedBy).toBeUndefined();
  });

  it("maps workspace_inactive to a generic 409 with stable guidance and status context", () => {
    const body = { error: "workspace_inactive", workspaceStatus: "deleting" };
    const err = apiErrorFromResponse({ status: 409, body });

    expect(err).toBeInstanceOf(AexApiError);
    expect(err).not.toBeInstanceOf(AexIdempotencyConflictError);
    expect(err.apiCode).toBe("workspace_inactive");
    expect(err.body).toEqual(body);
    expect(err.message).toBe(AEX_API_ERROR_MESSAGES.workspace_inactive);
    expect(AEX_API_ERROR_REMEDIES.workspace_inactive).toMatch(/active workspace/i);
  });

  it("does not misclassify checkpoint availability as an idempotency conflict", () => {
    const body = { error: "checkpoint_not_available", requestId: "req-checkpoint" };
    const err = apiErrorFromResponse({ status: 409, body });

    expect(err).toBeInstanceOf(AexApiError);
    expect(err).not.toBeInstanceOf(AexIdempotencyConflictError);
    expect(err.apiCode).toBe("checkpoint_not_available");
    expect(err.requestId).toBe("req-checkpoint");
    expect(err.message).toBe(AEX_API_ERROR_MESSAGES.checkpoint_not_available);
  });

  it("does not infer idempotency from an unknown 409 body", () => {
    const body = { error: "future_state_conflict" };
    const err = apiErrorFromResponse({ status: 409, body });

    expect(err).toBeInstanceOf(AexApiError);
    expect(err).not.toBeInstanceOf(AexIdempotencyConflictError);
    expect(err.apiCode).toBeUndefined();
    expect(err.body).toEqual(body);
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

  it.each([
    { status: 413, code: "event_archive_too_large" },
    { status: 503, code: "event_archive_deadline_exceeded" }
  ] as const)("maps archive HTTP $status to stable generic code $code", ({ status, code }) => {
    const err = apiErrorFromResponse({ status, body: { error: code, retryable: false } });

    expect(err).toBeInstanceOf(AexApiError);
    expect(err.apiCode).toBe(code);
    expect(err.message).toBe(AEX_API_ERROR_MESSAGES[code]);
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
        case "content_deleted":
          return "content_deleted";
        case "session_busy":
        case "checkpoint_not_available":
        case "session_not_terminal":
        case "session_terminal":
        case "event_archive_too_large":
        case "event_archive_deadline_exceeded":
        case "unknown_workspace":
        case "workspace_inactive":
        case "workspace_spend_cap_exceeded":
        case "workspace_cap_exceeded":
        case "insufficient_balance":
        case "subscription_past_due":
        case "quota_exhausted":
        case "depth_exceeded":
        case "out_of_memory":
        case "disk_full":
        case "upstream_error":
        case "internal_error":
          return "generic";
      }
    };
    expect(kindOf("not_found")).toBe("not_found");
  });
});
