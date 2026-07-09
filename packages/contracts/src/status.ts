export const SESSION_CONTROL_STATUSES = [
  "queued",
  "claiming",
  "provisioning",
  "session_created",
  "dispatched",
  "provider_running",
  "provider_idle",
  "provider_rescheduled",
  "idle",
  "suspending",
  "suspended",
  "deleting",
  "deleted",
  "expired",
  "cancelling",
  "capturing_outputs",
  "cleaning_up",
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "cleanup_failed"
] as const;

export type SessionControlStatus = typeof SESSION_CONTROL_STATUSES[number];

/**
 * The RESUMABLE / in-flight lifecycle half of a session's status. A session
 * whose status is one of these is either transitioning or parked resumable
 * (`idle`/`suspended`) — it is NOT a terminal outcome. The terminal outcome
 * half is {@link SESSION_TERMINAL_OUTCOMES}, derived from the session outcome SSoT.
 */
export const SESSION_LIFECYCLE_STATUSES = [
  "creating",
  "running",
  "idle",
  "suspending",
  "suspended",
  "cancelling",
  "deleting",
  "deleted",
  "expired"
] as const;

export type SessionLifecycleStatus = typeof SESSION_LIFECYCLE_STATUSES[number];

/**
 * The terminal OUTCOME half of a session's status — DERIVED from the session
 * outcome SSoT via `satisfies readonly SessionTurnTerminalOutcome[]`, so a new run
 * outcome fails to compile until it is accounted for here (and therefore in
 * {@link SESSION_STATUSES}). This is the compile-time binding that stops the
 * session and session terminal vocabularies from drifting: the bare session
 * `error` is gone — a failed turn is `failed`, a wall-clock kill `timed_out`,
 * a cancel `cancelled`, a clean finish `succeeded`.
 */
export const SESSION_TERMINAL_OUTCOMES = [
  "succeeded",
  "failed",
  "timed_out",
  "cancelled"
] as const satisfies readonly SessionTurnTerminalOutcome[];

export type SessionTerminalOutcome = typeof SESSION_TERMINAL_OUTCOMES[number];

/**
 * The full closed set of session statuses: the resumable lifecycle half, the
 * terminal-outcome half (bound to {@link SESSION_TURN_TERMINAL_OUTCOMES}), and the
 * first-class HITL write-gate `awaiting_approval`. Composed — never
 * hand-listed — so the outcome vocabulary can only be extended at the session SSoT.
 */
export const SESSION_STATUSES = [
  ...SESSION_LIFECYCLE_STATUSES,
  ...SESSION_TERMINAL_OUTCOMES,
  "awaiting_approval"
] as const;

export type SessionStatus = typeof SESSION_STATUSES[number];

/**
 * The terminal session statuses: the four outcomes plus the lifecycle-terminal
 * `deleted`/`expired`. `idle`/`suspended` are resumable (NOT terminal) and
 * `awaiting_approval` is a held gate (NOT terminal).
 */
const SESSION_TERMINAL_STATUSES = new Set<SessionStatus>([
  ...SESSION_TERMINAL_OUTCOMES,
  "deleted",
  "expired"
]);

/** True when a session status is terminal (an outcome, or deleted/expired). */
export function isTerminalSessionStatus(status: SessionStatus): boolean {
  return SESSION_TERMINAL_STATUSES.has(status);
}

export type SessionControlStatusKind = "active" | "terminal";

export const TERMINAL_SESSION_CONTROL_STATUSES = [
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "deleted",
  "expired",
  "cleanup_failed"
] as const satisfies readonly SessionControlStatus[];

const terminalSessionControlStatuses = new Set<SessionControlStatus>(TERMINAL_SESSION_CONTROL_STATUSES);

export function isTerminalSessionControlStatus(status: SessionControlStatus): boolean {
  return terminalSessionControlStatuses.has(status);
}

/**
 * The closed set of terminal OUTCOMES the session-lifecycle funnel writes via
 * `markTurnTerminal` (and that a `session/terminal` event carries). This is a
 * STRICT SUBSET of {@link TERMINAL_SESSION_CONTROL_STATUSES}: the read-terminal set also
 * includes `cleanup_failed`, which the funnel never writes as an outcome.
 * The platform lifecycle `TerminalSessionControlStatus` and the workflow `TerminalOutcome` both
 * derive from this so the four call sites can't drift.
 */
export const SESSION_TURN_TERMINAL_OUTCOMES = [
  "succeeded",
  "failed",
  "timed_out",
  "cancelled"
] as const satisfies readonly SessionControlStatus[];

export type SessionTurnTerminalOutcome = typeof SESSION_TURN_TERMINAL_OUTCOMES[number];

export function getSessionControlStatusKind(status: SessionControlStatus): SessionControlStatusKind {
  return isTerminalSessionControlStatus(status) ? "terminal" : "active";
}

export const CLEANUP_STATUSES = [
  "not_started",
  "pending",
  "running",
  "succeeded",
  "failed_retryable",
  "failed_terminal",
  "skipped"
] as const;

export type CleanupStatus = typeof CLEANUP_STATUSES[number];
