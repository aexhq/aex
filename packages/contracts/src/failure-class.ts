/**
 * SSoT for the CUSTOMER-FACING failure taxonomy carried by `RUN_ERROR`.
 *
 * `AexRunErrorData.failureClass` used to be an open `string`, so a customer
 * could not branch on WHY a session ended — every exhaustion looked like an
 * opaque `failed`. This tuple closes it. The hosted platform keeps a larger
 * PRIVATE taxonomy (orchestration detail) and projects it onto exactly this
 * union before the value reaches a customer; the platform's
 * `PUBLIC_FAILURE_CLASSES` is this array, imported, never a hand-copied mirror.
 *
 * Resource exhaustion is a first-class customer-facing outcome here:
 * `out_of_memory`, `disk_full`, and `quota_exhausted` say what ran out. There is
 * deliberately NO `resource_exhausted` — a code that teaches the customer
 * nothing is worse than the `failed` it replaces.
 */

export const AEX_FAILURE_CLASSES = [
  /** The session exceeded its wall-clock deadline. */
  "wall_clock_exceeded",
  /** The session exhausted its agent-turn budget. */
  "max_turns",
  /** The session exhausted its spend budget. */
  "budget_exhausted",
  /** A single turn exhausted its per-turn step budget. */
  "step_budget_exceeded",
  /** An agent step failed and could not be recovered. */
  "step_failed",
  /** Legacy public projection for already-persisted sessions; new runs never produce it. */
  "loop_detected",
  /** The submission was rejected as invalid before any work started. */
  "invalid_submission",
  /** The requested model is not servable. */
  "invalid_model",
  /** The workspace could not be restored to the state the session required. */
  "workspace_restore_incomplete",
  /** The provider's response stream broke mid-completion. */
  "llm_response_stream_failed",
  /** The platform had no capacity to place this session. */
  "capacity_exhausted",
  /** The runtime was killed for exceeding its memory allocation. */
  "out_of_memory",
  /** A write failed because the runtime's filesystem was full. */
  "disk_full",
  /** A metered usage grant was exhausted and there was no billable path. */
  "quota_exhausted",
  /** A transient provider fault; the same request may succeed on retry. */
  "transient-provider",
  /** A permanent provider fault; retrying the same request will not help. */
  "provider-permanent",
  /** The session was cancelled. */
  "cancelled",
  /** An unclassified platform fault. */
  "internal_error"
] as const;

/** The closed customer-facing failure taxonomy carried by `RUN_ERROR`. */
export type AexFailureClass = (typeof AEX_FAILURE_CLASSES)[number];

const FAILURE_CLASS_SET: ReadonlySet<string> = new Set(AEX_FAILURE_CLASSES);

/** Narrow an arbitrary value to a known {@link AexFailureClass}. */
export function isAexFailureClass(value: unknown): value is AexFailureClass {
  return typeof value === "string" && FAILURE_CLASS_SET.has(value);
}

/**
 * The subset that means "a resource ran out". Customers branch on this to
 * decide between retrying smaller, upgrading, and paging a human — the whole
 * reason the three codes exist as distinct values rather than one vague one.
 */
export const AEX_RESOURCE_EXHAUSTION_FAILURE_CLASSES = [
  "out_of_memory",
  "disk_full",
  "quota_exhausted"
] as const satisfies readonly AexFailureClass[];

export type AexResourceExhaustionFailureClass =
  (typeof AEX_RESOURCE_EXHAUSTION_FAILURE_CLASSES)[number];

const RESOURCE_EXHAUSTION_SET: ReadonlySet<string> = new Set(
  AEX_RESOURCE_EXHAUSTION_FAILURE_CLASSES
);

/** True when the failure was a resource running out rather than a logic fault. */
export function isResourceExhaustionFailureClass(
  value: unknown
): value is AexResourceExhaustionFailureClass {
  return typeof value === "string" && RESOURCE_EXHAUSTION_SET.has(value);
}
