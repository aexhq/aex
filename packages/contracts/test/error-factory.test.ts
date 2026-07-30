import { describe, expect, it } from "bun:test";
import {
  AEX_API_ERROR_CODES,
  AexApiError,
  AexAuthError,
  AexIdempotencyConflictError,
  AexNotFoundError,
  AexRateLimitError,
  apiErrorFromResponse,
  apiErrorKindForCode
} from "../src/index.js";

function envelope(
  code: (typeof AEX_API_ERROR_CODES)[number],
  details?: Record<string, unknown>
) {
  return {
    error: {
      code,
      message: `message:${code}`,
      requestId: "req_accepted_v1",
      retryable: code === "rate_limited",
      ...(details === undefined ? {} : { details })
    }
  };
}

describe("v1 wire error dispatch", () => {
  it("maps auth, idempotency, not-found, and rate-limit envelopes", () => {
    expect(apiErrorFromResponse({
      status: 403,
      body: envelope("insufficient_scope", { requiredScope: "billing:read" })
    })).toBeInstanceOf(AexAuthError);
    expect(apiErrorFromResponse({
      status: 409,
      body: envelope("operation_idempotency_conflict")
    })).toBeInstanceOf(AexIdempotencyConflictError);
    expect(apiErrorFromResponse({
      status: 404,
      body: envelope("file_not_found")
    })).toBeInstanceOf(AexNotFoundError);
    const limited = apiErrorFromResponse({
      status: 429,
      body: envelope("rate_limited", { retryAfterMs: 250 })
    });
    expect(limited).toBeInstanceOf(AexRateLimitError);
    expect((limited as AexRateLimitError).retryAfterMs).toBe(250);
  });

  it("keeps pause/account failures explicit on the generic API error", () => {
    const paused = apiErrorFromResponse({
      status: 402,
      body: envelope("account_paused", { minimumRestoreCents: "500" })
    });
    expect(paused).toBeInstanceOf(AexApiError);
    expect(paused.apiCode).toBe("account_paused");
    expect(paused.requestId).toBe("req_accepted_v1");
    expect(paused.body).toEqual(envelope("account_paused", {
      minimumRestoreCents: "500"
    }));
  });

  it("exhaustively classifies every stable code", () => {
    for (const code of AEX_API_ERROR_CODES) {
      expect(() => apiErrorKindForCode(code)).not.toThrow();
    }
  });
});
