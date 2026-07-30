/** Stable v1 wire-error identities. */
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
  "operation_idempotency_conflict",
  "invalid_cursor",
  "invalid_file_selection",
  "session_not_idle",
  "workspace_activation_required",
  "workspace_not_live",
  "session_deleting",
  "session_deleted",
  "deletion_in_progress",
  "operation_not_cancelable",
  "approval_not_found",
  "approval_already_resolved",
  "file_not_found",
  "export_not_found",
  "export_revoked",
  "invalid_range",
  "download_grant_expired",
  "telemetry_payload_too_large",
  "invalid_telemetry",
  "invalid_query",
  "invalid_metric_aggregation",
  "unsupported_export_signal",
  "telemetry_incomplete",
  "export_not_ready",
  "export_expired",
  "invalid_network_policy",
  "unsupported_package_ecosystem",
  "package_resolution_failed",
  "package_artifact_unavailable",
  "package_integrity_mismatch",
  "invalid_auto_topup_policy",
  "payment_method_required",
  "authentication_unavailable",
  "account_paused",
  "account_state_unavailable",
  "precondition_failed",
  "wrong_workspace_region",
  "observability_unavailable",
  "telemetry_quota_exceeded",
  "rate_limited",
  "upstream_error",
  "internal_error"
] as const;

export type AexApiErrorCode = (typeof AEX_API_ERROR_CODES)[number];

export const AEX_API_ERROR_MESSAGES: Record<AexApiErrorCode, string> = {
  unauthorized: "The request was not authenticated.",
  forbidden: "The API key is not permitted to perform this action.",
  insufficient_scope: "The API key is missing a scope this endpoint requires.",
  token_invalid: "The API key is not valid.",
  token_revoked: "The API key has been revoked.",
  token_expired: "The API key has expired.",
  malformed_token: "The API key is malformed.",
  not_found: "The requested resource was not found.",
  idempotency_conflict:
    "This idempotency key was already used with a different request body.",
  operation_idempotency_conflict:
    "This operation ID was already used with a different canonical intent.",
  invalid_cursor: "The cursor is malformed, expired, or does not match this query.",
  invalid_file_selection: "The file selection is invalid.",
  session_not_idle: "The session must be idle for this action.",
  workspace_activation_required:
    "The live workspace must be explicitly activated for this action.",
  workspace_not_live: "The session has no live workspace.",
  session_deleting: "Session deletion is in progress.",
  session_deleted: "The session has been deleted.",
  deletion_in_progress: "A deletion operation is already in progress.",
  operation_not_cancelable: "The operation cannot be canceled in its current state.",
  approval_not_found: "The approval was not found.",
  approval_already_resolved: "The approval has already been resolved.",
  file_not_found: "The selected file was not found.",
  export_not_found: "The telemetry export was not found.",
  export_revoked: "The telemetry export was revoked.",
  invalid_range: "The requested byte range is invalid.",
  download_grant_expired: "The download grant has expired.",
  telemetry_payload_too_large: "The telemetry batch exceeds an admitted payload bound.",
  invalid_telemetry: "The telemetry batch contains invalid observations.",
  invalid_query: "The observation query is not valid.",
  invalid_metric_aggregation:
    "The metric aggregation request is not valid for this instrument.",
  unsupported_export_signal:
    "The selected export format cannot represent one or more requested signals.",
  telemetry_incomplete:
    "The requested complete telemetry range contains a permanent gap.",
  export_not_ready: "The telemetry export is not ready for download.",
  export_expired: "The telemetry export has expired.",
  invalid_network_policy: "The requested network policy is invalid.",
  unsupported_package_ecosystem: "The package ecosystem is not supported.",
  package_resolution_failed: "The requested package could not be resolved.",
  package_artifact_unavailable: "The resolved package artifact is unavailable.",
  package_integrity_mismatch: "The package artifact failed integrity verification.",
  invalid_auto_topup_policy:
    "The automatic top-up policy is not a complete valid replacement.",
  payment_method_required:
    "A usable organization payment method is required for this billing action.",
  authentication_unavailable:
    "Current central authentication authority is temporarily unavailable.",
  account_paused:
    "The organization account is paused and this action is not pause-exempt.",
  account_state_unavailable:
    "The current organization account-state revision could not be established.",
  precondition_failed: "The supplied resource revision precondition does not match.",
  wrong_workspace_region:
    "The workspace belongs to a different immutable execution region.",
  observability_unavailable:
    "Durable regional observation admission or query is unavailable.",
  telemetry_quota_exceeded:
    "The telemetry request exceeds an effective workspace limit.",
  rate_limited: "Too many requests; retry after a bounded backoff.",
  upstream_error: "An upstream provider returned an error.",
  internal_error: "The aex API encountered an internal error."
};

export const AEX_API_ERROR_REMEDIES: Partial<Record<AexApiErrorCode, string>> = {
  insufficient_scope:
    "Mint an API key that includes the scope this endpoint requires.",
  idempotency_conflict:
    "Use a fresh idempotency key, or replay the byte-identical request.",
  operation_idempotency_conflict:
    "Use a fresh operation ID, or replay the same canonical intent.",
  workspace_activation_required:
    'Retry the live action explicitly with wake: "retained".',
  download_grant_expired: "Explicitly request a new download grant.",
  invalid_auto_topup_policy:
    "Replace the whole policy with valid whole-cent USD values.",
  payment_method_required:
    "Open an organization billing portal session and add a payment method.",
  account_paused:
    "Top up the reported minimum, then explicitly start the next action.",
  wrong_workspace_region:
    "Use the authoritative region and API URL returned with the workspace record.",
  token_invalid: "Check the workspace API key value.",
  token_expired: "Mint a new workspace API key.",
  token_revoked: "Mint a new workspace API key."
};

const API_ERROR_CODE_SET: ReadonlySet<string> = new Set(AEX_API_ERROR_CODES);

export function isAexApiErrorCode(value: unknown): value is AexApiErrorCode {
  return typeof value === "string" && API_ERROR_CODE_SET.has(value);
}
