/**
 * WS1 class-killer: the session terminal vocabulary is DERIVED from the run
 * outcome SSoT, so a new run outcome cannot be added without the session surface
 * gaining it. Runtime subset assert + a compile-time `satisfies` guard.
 */
import { describe, expect, it } from "vitest";
import {
  RUN_TERMINAL_OUTCOMES,
  SESSION_STATUSES,
  SESSION_LIFECYCLE_STATUSES,
  SESSION_TERMINAL_OUTCOMES,
  isTerminalSessionStatus,
  type RunTerminalOutcome,
  type SessionStatus
} from "../src/index.js";

describe("session/run terminal-outcome SSoT (WS1)", () => {
  it("RUN_TERMINAL_OUTCOMES is a runtime subset of SESSION_STATUSES", () => {
    for (const outcome of RUN_TERMINAL_OUTCOMES) {
      expect(SESSION_STATUSES as readonly string[]).toContain(outcome);
    }
  });

  it("SESSION_TERMINAL_OUTCOMES equals RUN_TERMINAL_OUTCOMES element-for-element", () => {
    expect([...SESSION_TERMINAL_OUTCOMES]).toEqual([...RUN_TERMINAL_OUTCOMES]);
  });

  it("recomposes SESSION_STATUSES = lifecycle ∪ terminal-outcomes ∪ awaiting_approval", () => {
    expect([...SESSION_STATUSES]).toEqual([
      ...SESSION_LIFECYCLE_STATUSES,
      ...SESSION_TERMINAL_OUTCOMES,
      "awaiting_approval"
    ]);
  });

  it("replaces the bare 'error' status with 'failed' and adds awaiting_approval", () => {
    expect(SESSION_STATUSES as readonly string[]).not.toContain("error");
    expect(SESSION_STATUSES as readonly string[]).toContain("failed");
    expect(SESSION_STATUSES as readonly string[]).toContain("awaiting_approval");
  });

  it("isTerminalSessionStatus: outcomes + deleted/expired are terminal; idle/running/awaiting_approval are not", () => {
    for (const terminal of ["succeeded", "failed", "timed_out", "cancelled", "deleted", "expired"] as SessionStatus[]) {
      expect(isTerminalSessionStatus(terminal)).toBe(true);
    }
    for (const live of ["idle", "running", "suspended", "awaiting_approval"] as SessionStatus[]) {
      expect(isTerminalSessionStatus(live)).toBe(false);
    }
  });

  it("[compile-time] a non-RunTerminalOutcome member fails the satisfies constraint", () => {
    // @ts-expect-error — "not_a_run_outcome" is not a RunTerminalOutcome, so the
    // `satisfies readonly RunTerminalOutcome[]` derivation fails to compile.
    const bad = ["succeeded", "not_a_run_outcome"] as const satisfies readonly RunTerminalOutcome[];
    expect(bad.length).toBe(2);
  });
});
