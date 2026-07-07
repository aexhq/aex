import { describe, expect, it } from "vitest";
import {
  abortableSleep,
  computeRetryBackoffDelayMs,
  computeRetryDelayMs,
  isRateLimitHttpStatus,
  isRetryableHttpStatus,
  parseRetryAfterMs,
  RATE_LIMIT_HTTP_STATUS,
  RETRYABLE_HTTP_STATUS
} from "../src/internal.js";

describe("internal retry core", () => {
  it("classifies retryable and rate-limit HTTP statuses", () => {
    expect([...RETRYABLE_HTTP_STATUS].sort((a, b) => a - b)).toEqual([429, 500, 502, 503, 504, 529]);
    expect([...RATE_LIMIT_HTTP_STATUS].sort((a, b) => a - b)).toEqual([429, 503, 529]);

    for (const status of [429, 500, 502, 503, 504, 529]) {
      expect(isRetryableHttpStatus(status)).toBe(true);
    }
    for (const status of [200, 400, 401, 403, 404, 409, 422, 501]) {
      expect(isRetryableHttpStatus(status)).toBe(false);
    }
    expect(isRateLimitHttpStatus(429)).toBe(true);
    expect(isRateLimitHttpStatus(503)).toBe(true);
    expect(isRateLimitHttpStatus(529)).toBe(true);
    expect(isRateLimitHttpStatus(500)).toBe(false);
  });

  it("parses Retry-After seconds and HTTP dates", () => {
    const now = Date.UTC(2026, 0, 1, 0, 0, 0);
    expect(parseRetryAfterMs(" 3 ")).toBe(3000);
    expect(parseRetryAfterMs(new Date(now + 12_000).toUTCString(), now)).toBe(12_000);
    expect(parseRetryAfterMs(new Date(now - 1000).toUTCString(), now)).toBe(0);
    expect(parseRetryAfterMs(null)).toBeUndefined();
    expect(parseRetryAfterMs("soon")).toBeUndefined();
  });

  it("computes full-jitter exponential backoff with injectable random", () => {
    const config = { initialDelayMs: 100, maxDelayMs: 1000 };
    expect(computeRetryBackoffDelayMs(config, 1, () => 1)).toBe(100);
    expect(computeRetryBackoffDelayMs(config, 3, () => 0.5)).toBe(200);
    expect(computeRetryBackoffDelayMs(config, 6, () => 1)).toBe(1000);
  });

  it("combines Retry-After as a floor while sampling jitter once", () => {
    let calls = 0;
    const delay = computeRetryDelayMs(
      { initialDelayMs: 100, maxDelayMs: 1000 },
      3,
      () => {
        calls += 1;
        return 0.5;
      },
      250
    );
    expect(delay).toBe(250);
    expect(calls).toBe(1);
  });

  it("rejects abortable sleep with the caller's abort reason", async () => {
    const controller = new AbortController();
    const reason = new Error("stop");
    controller.abort(reason);
    await expect(abortableSleep(1000, controller.signal)).rejects.toBe(reason);
  });
});
