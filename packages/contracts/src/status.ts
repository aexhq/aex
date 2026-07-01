export const RUN_STATUSES = [
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

export type RunStatus = typeof RUN_STATUSES[number];

export const SESSION_STATUSES = [
  "creating",
  "running",
  "idle",
  "suspending",
  "suspended",
  "cancelling",
  "deleting",
  "deleted",
  "error"
] as const;

export type SessionStatus = typeof SESSION_STATUSES[number];

export type RunStatusKind = "active" | "terminal";

export const TERMINAL_RUN_STATUSES = [
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "cleanup_failed"
] as const satisfies readonly RunStatus[];

const terminalRunStatuses = new Set<RunStatus>(TERMINAL_RUN_STATUSES);

export function isTerminalRunStatus(status: RunStatus): boolean {
  return terminalRunStatuses.has(status);
}

/**
 * The closed set of terminal OUTCOMES the run-lifecycle funnel writes via
 * `markRunTerminal` (and that a `run/terminal` event carries). This is a
 * STRICT SUBSET of {@link TERMINAL_RUN_STATUSES}: the read-terminal set also
 * includes `cleanup_failed`, which the funnel never writes as an outcome.
 * The platform lifecycle `TerminalRunStatus` and the workflow `TerminalOutcome` both
 * derive from this so the four call sites can't drift.
 */
export const RUN_TERMINAL_OUTCOMES = [
  "succeeded",
  "failed",
  "timed_out",
  "cancelled"
] as const satisfies readonly RunStatus[];

export type RunTerminalOutcome = typeof RUN_TERMINAL_OUTCOMES[number];

export function getRunStatusKind(status: RunStatus): RunStatusKind {
  return isTerminalRunStatus(status) ? "terminal" : "active";
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
