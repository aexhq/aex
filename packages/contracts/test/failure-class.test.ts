import { describe, expect, it } from "bun:test";
import {
  AEX_FAILURE_CLASSES,
  AEX_RESOURCE_EXHAUSTION_FAILURE_CLASSES,
  isAexFailureClass,
  isResourceExhaustionFailureClass,
  type AexFailureClass
} from "../src/failure-class.js";
import { AEX_API_ERROR_CODES, type AexApiErrorCode } from "../src/error-codes.js";
import { classifyAexEvent, isRunError, type AexEventBase } from "../src/event-envelope.js";
import * as root from "../src/index.js";

const EXPECTED = [
  "wall_clock_exceeded",
  "max_turns",
  "budget_exhausted",
  "step_budget_exceeded",
  "step_failed",
  "loop_detected",
  "invalid_submission",
  "invalid_model",
  "workspace_restore_incomplete",
  "llm_response_stream_failed",
  "capacity_exhausted",
  "out_of_memory",
  "disk_full",
  "quota_exhausted",
  "transient-provider",
  "provider-permanent",
  "cancelled",
  "internal_error"
] as const satisfies readonly AexFailureClass[];

function runError(failureClass: unknown): AexEventBase {
  return {
    specversion: "1.0",
    id: "evt-1",
    source: "runtime",
    type: "RUN_ERROR",
    subject: "ses_1",
    time: new Date(0).toISOString(),
    sequence: 1,
    channel: "event",
    sourceSeq: 1,
    emittedAt: 0,
    data: { outcome: "failed", failureClass, failureMessage: "boom" }
  } as unknown as AexEventBase;
}

describe("public failure taxonomy", () => {
  it("pins the exact closed union and re-exports it from the package root", () => {
    expect(AEX_FAILURE_CLASSES).toEqual(EXPECTED);
    expect(new Set(AEX_FAILURE_CLASSES).size).toBe(AEX_FAILURE_CLASSES.length);
    expect(root.AEX_FAILURE_CLASSES).toBe(AEX_FAILURE_CLASSES);
    expect(root.isAexFailureClass).toBe(isAexFailureClass);
  });

  /**
   * Resource exhaustion is a first-class CUSTOMER-facing outcome: an OOM must
   * arrive as `out_of_memory`, not as an opaque `failed`. `resource_exhausted`
   * is deliberately absent — a code that names nothing teaches nothing.
   */
  it("carries the three exhaustion outcomes and refuses the vague one", () => {
    expect(AEX_RESOURCE_EXHAUSTION_FAILURE_CLASSES).toEqual(["out_of_memory", "disk_full", "quota_exhausted"]);
    for (const failureClass of AEX_RESOURCE_EXHAUSTION_FAILURE_CLASSES) {
      expect(isAexFailureClass(failureClass), failureClass).toBe(true);
      expect(isResourceExhaustionFailureClass(failureClass), failureClass).toBe(true);
    }
    expect(isAexFailureClass("resource_exhausted")).toBe(false);
    expect(isResourceExhaustionFailureClass("step_failed")).toBe(false);
    expect(isAexFailureClass("")).toBe(false);
    expect(isAexFailureClass(undefined)).toBe(false);
  });

  /**
   * The whole point of closing the union: an unknown class is a CONTRACT
   * violation the guard names, not a value that silently reaches a `switch`
   * the customer wrote against the documented set.
   */
  it("rejects a RUN_ERROR whose failureClass is outside the union", () => {
    for (const failureClass of AEX_FAILURE_CLASSES) {
      expect(isRunError(runError(failureClass)), failureClass).toBe(true);
    }
    for (const bad of ["session_failed", "resource_exhausted", "provider_permanent", "", 7, null]) {
      const classified = classifyAexEvent(runError(bad));
      expect(classified, JSON.stringify(bad)).toMatchObject({
        kind: "malformed_known",
        issue: { type: "RUN_ERROR", path: "data.failureClass" }
      });
      expect(isRunError(runError(bad)), JSON.stringify(bad)).toBe(false);
    }
  });

  /**
   * The three exhaustion outcomes exist in BOTH stable vocabularies: as a
   * terminal `failureClass` (the session died of it) and as an API error code
   * (admission refused it up front). A customer branches on one identifier.
   */
  it("keeps the exhaustion vocabulary aligned with the stable API error codes", () => {
    const codes = new Set<AexApiErrorCode>(AEX_API_ERROR_CODES);
    for (const failureClass of AEX_RESOURCE_EXHAUSTION_FAILURE_CLASSES) {
      expect(codes.has(failureClass as AexApiErrorCode), failureClass).toBe(true);
    }
    expect(codes.has("resource_exhausted" as AexApiErrorCode)).toBe(false);
    // `rate_limited` is the ONE rate-limit code. A second near-duplicate spelling
    // teaches customers to branch on two identifiers for one condition.
    expect(codes.has("rate_limited")).toBe(true);
    expect(codes.has("rate_limit_exceeded" as AexApiErrorCode)).toBe(false);
  });
});
