import { describe, expect, test } from "bun:test";

import {
  centsToUsd,
  classifyFailure,
  parseRetryAfter,
  readCoverage,
  readFailure,
  usageThrough,
  type ObservationCoverage,
} from "../src/ui/panel";

function envelope(code: string, retryable = false): unknown {
  return { error: { code, message: "stated plainly", requestId: "req_1", retryable } };
}

describe("failure classification", () => {
  test("a paused account is a product state, not an error", () => {
    const state = classifyFailure(402, envelope("account_paused"), null);
    expect(state.kind).toBe("paused");
  });

  test("an unavailable observation store is retryable and separate from a failure", () => {
    const state = classifyFailure(503, envelope("observability_unavailable", true), 30_000);
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

const WHOLE: ObservationCoverage = {
  accepted: "1000",
  indexed: "1000",
  snapshot: "1000",
  earliestReplay: "0",
  caughtUp: true,
  complete: true,
  missingIntervals: [],
  unboundedGaps: [],
};

describe("coverage", () => {
  test("a whole, caught-up window is complete", () => {
    expect(readCoverage(WHOLE)).toEqual({ kind: "complete", completeThrough: 1000 });
  });

  test("a whole window that trails admission reports the lag, not a hole", () => {
    const verdict = readCoverage({ ...WHOLE, caughtUp: false, accepted: "9000" });
    expect(verdict).toEqual({ kind: "behind", completeThrough: 1000, lagMs: 8000 });
  });

  test("a hole outranks catching up, and carries the gap identities", () => {
    const hole = { gapId: "gap_1", range: { gte: "2026-01-01T00:00:00.000Z", lt: "2026-01-01T01:00:00.000Z" } };
    const verdict = readCoverage({ ...WHOLE, complete: false, missingIntervals: [hole] });
    expect(verdict).toEqual({
      kind: "incomplete",
      completeThrough: 1000,
      lagMs: 0,
      holes: [hole],
      unboundedGaps: [],
    });
  });

  test("an unbounded gap alone is enough to make a window incomplete", () => {
    const verdict = readCoverage({ ...WHOLE, unboundedGaps: ["gap_2"] });
    expect(verdict.kind).toBe("incomplete");
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
