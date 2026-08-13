import { describe, expect, test } from "bun:test";

import {
  centsToUsd,
  classifyFailure,
  parseRetryAfter,
  readFailure,
  usageThrough,
} from "../src/ui/panel";

function envelope(code: string, retryable = false): unknown {
  return { error: { code, message: "stated plainly", requestId: "req_1", retryable } };
}

describe("failure classification", () => {
  test("a paused account is a product state, not an error", () => {
    const state = classifyFailure(402, envelope("account_paused"), null);
    expect(state.kind).toBe("paused");
  });

  test("an unavailable upstream is retryable and separate from a failure", () => {
    const state = classifyFailure(503, envelope("upstream_error", true), 30_000);
    expect(state).toEqual(expect.objectContaining({ kind: "unavailable", retryAfterMs: 30_000 }));
  });

  test("each remaining wire status maps to its own remedy", () => {
    expect(classifyFailure(401, envelope("unauthenticated"), null).kind).toBe("expired");
    expect(classifyFailure(403, envelope("insufficient_scope"), null).kind).toBe("denied");
    expect(classifyFailure(404, envelope("not_found"), null).kind).toBe("missing");
    expect(classifyFailure(410, envelope("session_deleted"), null).kind).toBe("missing");
    expect(classifyFailure(429, envelope("rate_limited", true), 1_000).kind).toBe("throttled");
    expect(classifyFailure(400, envelope("invalid_query"), null).kind).toBe("failed");
    expect(classifyFailure(500, envelope("internal_error"), null).kind).toBe("unavailable");
  });

  test("an unreadable envelope becomes a typed failure rather than a blank message", () => {
    const failure = readFailure({ nonsense: true });
    expect(failure.code).toBe("invalid_error_envelope");
    expect(failure.retryable).toBe(false);
  });

  test("retry-after is read as seconds and rejected when it is not a number", () => {
    expect(parseRetryAfter("30")).toBe(30_000);
    expect(parseRetryAfter("Wed, 21 Oct 2026 07:28:00 GMT")).toBeNull();
    expect(parseRetryAfter(null)).toBeNull();
  });
});

describe("usage coverage honesty", () => {
  test("the shared service frontier is the earliest across every category", () => {
    expect(
      usageThrough([
        { category: "compute", region: "eu-west-1", serviceThrough: "2026-01-02T00:00:00.000Z" },
        { category: "storage", region: "eu-west-1", serviceThrough: "2026-01-01T00:00:00.000Z" },
      ]),
    ).toBe("2026-01-01T00:00:00.000Z");
  });

  test("an empty or partial frontier set cannot claim a shared coverage instant", () => {
    expect(usageThrough([])).toBeNull();
    expect(usageThrough([
      { category: "compute", region: "eu-west-1", serviceThrough: "2026-01-02T00:00:00.000Z" },
      { category: "storage", region: "eu-west-1" },
    ])).toBeNull();
  });

  test("cents are rendered as money and a malformed amount is never rendered as zero", () => {
    expect(centsToUsd("12345")).toBe("$123.45");
    expect(centsToUsd("not-a-number")).toBe("—");
  });
});
