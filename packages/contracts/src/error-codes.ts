/**
 * SSoT for the platform's STABLE API error codes.
 *
 * The server (aex-platform `api.ts`) imports this table instead of keeping its
 * own private copy, so a route emitting a code absent from the union fails to
 * compile and adding a code without a message is caught. The SDK error factory
 * ({@link import("./error-factory.js").apiErrorFromResponse}) dispatches on the
 * code to a typed subclass; the CLI maps it to a human remedy.
 *
 * The code is the stable, machine-branchable identity of a failure — distinct
 * from the human `message`. `idempotency_conflict` and `insufficient_scope`
 * (previously message-less bare 409/403 bodies) are first-class here.
 */

export const AEX_API_ERROR_CODES = [
  "unauthorized",
  "forbidden",
  "insufficient_scope",
  "token_invalid",
  "token_revoked",
  "token_expired",
  "malformed_token",
  "not_found",
  "idempotency_conflict",
  "session_busy",
  "session_not_terminal",
  "session_terminal",
  "event_archive_too_large",
  "event_archive_deadline_exceeded",
  "unknown_workspace",
  "workspace_inactive",
  "workspace_concurrency_exceeded",
  "workspace_submit_rate_exceeded",
  "workspace_spend_cap_exceeded",
  "insufficient_balance",
  "rate_limited",
  "upstream_error",
  "internal_error"
] as const;

export type AexApiErrorCode = (typeof AEX_API_ERROR_CODES)[number];

/**
 * Human message per stable code. A `Record` (not `Partial`) so a code added to
 * {@link AEX_API_ERROR_CODES} without a message is a COMPILE error. These are
 * the fallback messages the factory uses when the server sends a BARE code with
 * no `message` detail.
 */
export const AEX_API_ERROR_MESSAGES: Record<AexApiErrorCode, string> = {
  unauthorized: "The request was not authenticated.",
  forbidden: "The API key is not permitted to perform this action.",
  insufficient_scope: "The API key is missing a scope this endpoint requires.",
  token_invalid: "The API key is not valid.",
  token_revoked: "The API key has been revoked.",
  token_expired: "The API key has expired.",
  malformed_token: "The API key is malformed.",
  not_found: "The requested resource was not found.",
  idempotency_conflict: "This idempotency key was already used with a different request body.",
  session_busy: "The session is busy handling another turn.",
  session_not_terminal: "The session has not reached a terminal state yet.",
  session_terminal: "The session has ended and cannot perform this action.",
  event_archive_too_large: "The session event history is too large for synchronous bulk export.",
  event_archive_deadline_exceeded: "The synchronous session event export exceeded its deadline.",
  unknown_workspace: "The workspace could not be resolved from the API key.",
  workspace_inactive: "The workspace no longer accepts new session work.",
  workspace_concurrency_exceeded: "The workspace has reached its concurrent-run limit.",
  workspace_submit_rate_exceeded: "The workspace submit-rate limit was exceeded.",
  workspace_spend_cap_exceeded: "The workspace monthly spend cap was reached.",
  insufficient_balance: "The workspace prepaid balance is insufficient to submit this session.",
  rate_limited: "Too many requests — retry after a short backoff.",
  upstream_error: "An upstream provider returned an error.",
  internal_error: "The aex API encountered an internal error."
};

/** Optional per-code fix hint the CLI surfaces alongside the error message. */
export const AEX_API_ERROR_REMEDIES: Partial<Record<AexApiErrorCode, string>> = {
  insufficient_scope: "Mint an API key that includes the scope this endpoint requires.",
  idempotency_conflict:
    "Use a fresh idempotency key, or resubmit the byte-identical request body to replay the original result.",
  token_invalid: "Check the API key value and that its plane matches your baseUrl.",
  token_expired: "Mint a new API key.",
  token_revoked: "Mint a new API key.",
  session_terminal: "Create or open an active session before retrying this action.",
  event_archive_too_large: "Traverse the event history with session.events.iterate().",
  event_archive_deadline_exceeded: "Traverse the event history with session.events.iterate().",
  workspace_inactive: "Use an active workspace; a workspace being deleted cannot accept new session work.",
  workspace_spend_cap_exceeded: "Raise the workspace spend cap or wait for the next billing cycle.",
  insufficient_balance: "Top up the workspace balance or add a payment method.",
  workspace_concurrency_exceeded: "Wait for in-flight sessions to finish or raise the concurrency limit.",
  workspace_submit_rate_exceeded: "Slow the submit rate or raise the workspace submit-rate limit."
};

const API_ERROR_CODE_SET: ReadonlySet<string> = new Set(AEX_API_ERROR_CODES);

/** Narrow an arbitrary value to a known {@link AexApiErrorCode}. */
export function isAexApiErrorCode(value: unknown): value is AexApiErrorCode {
  return typeof value === "string" && API_ERROR_CODE_SET.has(value);
}
