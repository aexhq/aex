/**
 * The RESUMABLE / in-flight lifecycle half of a session's status. A session
 * whose status is one of these is either transitioning, parked and resumable,
 * or being removed. A prior run's verdict is not projected into this field;
 * read `lastRun.outcome` or the terminal RUN event instead. In particular,
 * `error` means the most recent run failed while the thread remains resumable.
 */
export const SESSION_LIFECYCLE_STATUSES = [
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
] as const;

export type SessionLifecycleStatus = typeof SESSION_LIFECYCLE_STATUSES[number];

/**
 * Terminal RUN outcomes. These describe a completed run, not the lifecycle
 * status of its resumable session thread.
 */
export const SESSION_TERMINAL_OUTCOMES = [
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "interrupted"
] as const;

export type SessionTerminalOutcome = typeof SESSION_TERMINAL_OUTCOMES[number];

/**
 * The full closed set of session lifecycle statuses. Run outcomes deliberately
 * remain separate so a resumable session never reports `succeeded`/`failed`.
 */
export const SESSION_STATUSES = SESSION_LIFECYCLE_STATUSES;

export type SessionStatus = SessionLifecycleStatus;

/**
 * Only removal makes a session thread terminal. Run outcomes are intentionally
 * not session statuses.
 */
const SESSION_TERMINAL_STATUSES = new Set<SessionStatus>([
  "deleted",
  "expired"
]);

/** True when a session thread has been deleted or expired. */
export function isTerminalSessionStatus(status: SessionStatus): boolean {
  return SESSION_TERMINAL_STATUSES.has(status);
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
