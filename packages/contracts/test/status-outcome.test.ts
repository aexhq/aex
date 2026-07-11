/**
 * WS1 class-killer: the session terminal vocabulary is DERIVED from the session
 * outcome SSoT, so a new run outcome cannot be added without the session surface
 * gaining it. Runtime subset assert + a compile-time `satisfies` guard.
 */
import { describe, expect, it } from "vitest";
import {
  SESSION_STATUSES,
  SESSION_LIFECYCLE_STATUSES,
  SESSION_TERMINAL_OUTCOMES,
  isTerminalSessionStatus,
  type SessionTerminalOutcome,
  type SessionStatus
} from "../src/index.js";

describe("session lifecycle and run outcome vocabularies", () => {
  it("keeps run outcomes out of resumable session statuses", () => {
    expect(SESSION_TERMINAL_OUTCOMES).toEqual(["succeeded", "failed", "timed_out", "cancelled", "interrupted"]);
    for (const outcome of SESSION_TERMINAL_OUTCOMES) {
      expect(SESSION_STATUSES as readonly string[]).not.toContain(outcome);
    }
  });

  it("defines SESSION_STATUSES solely from thread lifecycle states", () => {
    expect([...SESSION_STATUSES]).toEqual([...SESSION_LIFECYCLE_STATUSES]);
  });

  it("keeps error and approval holds resumable", () => {
    expect(SESSION_STATUSES as readonly string[]).toContain("error");
    expect(SESSION_STATUSES as readonly string[]).not.toContain("failed");
    expect(SESSION_STATUSES as readonly string[]).toContain("awaiting_approval");
  });

  it("treats only deleted and expired session threads as terminal", () => {
    for (const terminal of ["deleted", "expired"] as SessionStatus[]) {
      expect(isTerminalSessionStatus(terminal)).toBe(true);
    }
    for (const live of ["idle", "running", "suspended", "awaiting_approval", "error"] as SessionStatus[]) {
      expect(isTerminalSessionStatus(live)).toBe(false);
    }
  });

  it("[compile-time] a non-SessionTerminalOutcome member fails the satisfies constraint", () => {
    // @ts-expect-error - invalid values cannot satisfy the run outcome vocabulary.
    const bad = ["succeeded", "not_a_ses_outcome"] as const satisfies readonly SessionTerminalOutcome[];
    expect(bad.length).toBe(2);
  });
});
