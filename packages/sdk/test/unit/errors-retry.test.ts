import { describe, expect, test } from "bun:test";

import {
  AexApiError,
  AexAuthError,
  AexConflictError,
  AexInternalError,
  AexNotFoundError,
  AexPreconditionError,
  AexQuotaError,
  AexStateError,
  AexUnavailableError,
  AexValidationError,
  RETRY_POLICY,
  apiErrorFromResponse,
  executeWithRetry,
} from "../../src/index.js";

const cases = [
  ["unauthenticated", "auth", AexAuthError],
  ["not_found", "not_found", AexNotFoundError],
  ["idempotency_conflict", "conflict", AexConflictError],
  ["precondition_failed", "precondition", AexPreconditionError],
  ["invalid_request", "validation", AexValidationError],
  ["rate_limited", "quota", AexQuotaError],
  ["session_not_idle", "state", AexStateError],
  ["upstream_error", "unavailable", AexUnavailableError],
  ["internal_error", "internal", AexInternalError],
] as const;

describe("error classification", () => {
  test.each(cases)("maps %s through the total class table", (code, errorClass, ctor) => {
    const error = apiErrorFromResponse(
      "session_get",
      400,
      { error: { code, class: errorClass, message: "safe", requestId: "req_1", retryable: false } },
      new Headers(),
    );
    expect(error).toBeInstanceOf(ctor);
    expect(error.requestId).toBe("req_1");
    expect(error.retryable).toBe(false);
  });

  test("preserves an unrecognised code on the base class", () => {
    const error = apiErrorFromResponse(
      "session_get",
      599,
      { error: { code: "future_error", class: "future", message: "safe", requestId: "req_2", retryable: true } },
      new Headers(),
    );
    expect(error.constructor).toBe(AexApiError);
    expect(error.code).toBe("future_error");
  });
});

describe("retry policy", () => {
  test("uses route safeRetry, one identity, retry-after as a floor, and four attempts", async () => {
    const delays: number[] = [];
    const identities: string[] = [];
    let attempts = 0;
    const value = await executeWithRetry({
      route: { safeRetry: true },
      identity: "idk_same",
      random: () => 0,
      sleep: async (delay) => void delays.push(delay),
      attempt: async ({ identity }) => {
        attempts += 1;
        identities.push(identity);
        if (attempts < RETRY_POLICY.maxAttempts) {
          throw new AexUnavailableError({
            status: 503,
            code: "upstream_error",
            requestId: `req_${attempts}`,
            retryable: true,
            retryAfterMs: 700,
            message: "try later",
          });
        }
        return "ok";
      },
    });
    expect(value).toBe("ok");
    expect(attempts).toBe(4);
    expect(delays).toEqual([700, 700, 700]);
    expect(new Set(identities)).toEqual(new Set(["idk_same"]));
  });

  test("does not retry an unsafe route or after a streamed byte", async () => {
    for (const [safeRetry, streamedBytes] of [[false, 0], [true, 1]] as const) {
      let attempts = 0;
      await expect(
        executeWithRetry({
          route: { safeRetry },
          identity: "idk_once",
          streamedBytes,
          sleep: async () => undefined,
          attempt: async () => {
            attempts += 1;
            throw new TypeError("network");
          },
        }),
      ).rejects.toThrow();
      expect(attempts).toBe(1);
    }
  });
});
