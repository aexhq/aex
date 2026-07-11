/** Internal orchestration states. Public session reads use `SessionStatus`. */
export const SESSION_WORKFLOW_STATUSES = [
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
  "capturing_files",
  "cleaning_up",
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "interrupted",
  "cleanup_failed"
] as const;

export type SessionWorkflowStatus = (typeof SESSION_WORKFLOW_STATUSES)[number];
export type SessionWorkflowStatusKind = "active" | "terminal";

export const TERMINAL_SESSION_WORKFLOW_STATUSES = [
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "interrupted",
  "deleted",
  "expired",
  "cleanup_failed"
] as const satisfies readonly SessionWorkflowStatus[];

const terminalSessionWorkflowStatuses = new Set<SessionWorkflowStatus>(TERMINAL_SESSION_WORKFLOW_STATUSES);

export function isTerminalSessionWorkflowStatus(status: SessionWorkflowStatus): boolean {
  return terminalSessionWorkflowStatuses.has(status);
}

export function getSessionWorkflowStatusKind(status: SessionWorkflowStatus): SessionWorkflowStatusKind {
  return isTerminalSessionWorkflowStatus(status) ? "terminal" : "active";
}
