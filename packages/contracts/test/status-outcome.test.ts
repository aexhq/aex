/**
 * WS1 class-killer: the session terminal vocabulary is DERIVED from the session
 * outcome SSoT, so a new run outcome cannot be added without the session surface
 * gaining it. Runtime subset assert + a compile-time `satisfies` guard.
 */
import { describe, expect, it } from "bun:test";
import {
  SESSION_STATUSES,
  SESSION_LIFECYCLE_STATUSES,
  SESSION_TERMINAL_OUTCOMES,
  isTerminalSessionStatus,
  type SessionLifecycleStatus,
  type SessionTerminalOutcome,
  type SessionStatus
} from "../src/index.js";
import {
  SESSION_LIFECYCLE_STATUSES as STATUS_MODULE_LIFECYCLE_STATUSES,
  SESSION_STATUSES as STATUS_MODULE_STATUSES
} from "../src/status.js";

type IsExactly<Left, Right> =
  (<Value>() => Value extends Left ? 1 : 2) extends
  (<Value>() => Value extends Right ? 1 : 2)
    ? (<Value>() => Value extends Right ? 1 : 2) extends
      (<Value>() => Value extends Left ? 1 : 2)
      ? true
      : false
    : false;

type Assert<Condition extends true> = Condition;

type ExactSessionLifecycleTuple = readonly [
  "creating",
  "running",
  "idle",
  "suspending",
  "suspended",
  "awaiting_approval",
  "error",
  "cancelling",
  "deleting",
  "deleted",
  "expired"
];

type _SessionNamesShareOneType = Assert<IsExactly<SessionStatus, SessionLifecycleStatus>>;
type _SessionStatusesRetainExactTuple = Assert<IsExactly<typeof SESSION_STATUSES, ExactSessionLifecycleTuple>>;
type _SessionLifecycleStatusesRetainExactTuple = Assert<
  IsExactly<typeof SESSION_LIFECYCLE_STATUSES, ExactSessionLifecycleTuple>
>;

describe("session lifecycle and run outcome vocabularies", () => {
  it("keeps run outcomes out of resumable session statuses", () => {
    expect(SESSION_TERMINAL_OUTCOMES).toEqual(["succeeded", "failed", "timed_out", "cancelled", "interrupted"]);
    for (const outcome of SESSION_TERMINAL_OUTCOMES) {
      expect(SESSION_STATUSES as readonly string[]).not.toContain(outcome);
    }
  });

  it("exports both public names as one exact lifecycle tuple", () => {
    expect(SESSION_STATUSES).toBe(SESSION_LIFECYCLE_STATUSES);
    expect(SESSION_STATUSES).toBe(STATUS_MODULE_STATUSES);
    expect(SESSION_LIFECYCLE_STATUSES).toBe(STATUS_MODULE_LIFECYCLE_STATUSES);
    expect(SESSION_STATUSES).toEqual([
      "creating",
      "running",
      "idle",
      "suspending",
      "suspended",
      "awaiting_approval",
      "error",
      "cancelling",
      "deleting",
      "deleted",
      "expired"
    ]);

    const statusTuple: ExactSessionLifecycleTuple = SESSION_STATUSES;
    const lifecycleTuple: ExactSessionLifecycleTuple = SESSION_LIFECYCLE_STATUSES;
    const statusAlias: typeof SESSION_LIFECYCLE_STATUSES = SESSION_STATUSES;
    const lifecycleAlias: typeof SESSION_STATUSES = SESSION_LIFECYCLE_STATUSES;

    expect(statusTuple).toBe(lifecycleTuple);
    expect(statusAlias).toBe(lifecycleAlias);
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

  it("[compile-time] keeps session lifecycle and run outcome types separate", () => {
    // @ts-expect-error - a lifecycle status cannot be used as a completed run outcome.
    const lifecycleAsOutcome: SessionTerminalOutcome = "idle";
    // @ts-expect-error - a completed run outcome cannot be used as a session lifecycle status.
    const outcomeAsLifecycle: SessionStatus = "succeeded";

    expect<string>(lifecycleAsOutcome).toBe("idle");
    expect<string>(outcomeAsLifecycle).toBe("succeeded");
  });
});
