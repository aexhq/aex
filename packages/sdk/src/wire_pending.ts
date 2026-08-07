/**
 * TODO(cross-stream): replaced by `packages/sdk/src/generated/index.ts` at merge.
 * The contracts stream owns the generated TypeScript route and error vocabulary.
 *
 * Every row below is a verbatim projection of the checked-in contract registries
 * `api/generated/registries/routes.json` and `.../errors.json`. It carries the
 * bounded subset the implemented clients and the dashboard BFF need, not the full
 * 146-operation table. `packages/sdk/test/unit/contract-registry.test.ts` fails if
 * any field here drifts from the registry, so the duplication cannot rot silently.
 */

export type RouteId =
  | "account_get"
  | "api_key_create"
  | "api_key_revoke"
  | "api_keys_list"
  | "billing_auto_topup_policy_get"
  | "billing_auto_topup_policy_put"
  | "billing_balance_get"
  | "billing_portal_session_create"
  | "billing_statement_download_create"
  | "billing_statements_list"
  | "billing_top_up_checkout_create"
  | "dashboard_bootstrap_get"
  | "observations_events_query"
  | "observations_metrics_aggregate"
  | "observations_traces_query"
  | "organization_create"
  | "registry_files_download_create"
  | "registry_files_list"
  | "registry_instructions_list"
  | "registry_mcp_servers_list"
  | "registry_skills_list"
  | "registry_tools_list"
  | "secret_delete"
  | "secret_put"
  | "secret_revoke"
  | "secrets_list"
  | "session_approval_respond"
  | "session_approvals_list"
  | "session_create"
  | "session_files_persisted_download_create"
  | "session_files_persisted_list"
  | "session_get"
  | "session_observations_events_query"
  | "session_observations_trace_get"
  | "session_runs_list"
  | "sessions_list"
  | "telemetry_gaps_query"
  | "usage_query"
  | "workspace_create"
  | "workspace_get"
  | "workspace_limits_list";

export interface RouteDescriptor {
  readonly id: RouteId;
  readonly method: "GET" | "POST" | "PUT" | "DELETE";
  readonly path: string;
  readonly plane: "central" | "regional";
  readonly safeRetry: boolean;
  readonly idempotency: "none" | "idempotency_key" | "operation_id";
  readonly transport: "unary" | "ndjson";
  /** Path placeholders, in the order the contract declares them. */
  readonly pathParams: readonly string[];
  /** The closed set of query parameters the operation accepts. */
  readonly queryParams: readonly string[];
  /** Whether the operation still answers while the account is paused. */
  readonly pauseExempt: boolean;
}

export const ROUTES: Readonly<Record<RouteId, RouteDescriptor>> = Object.freeze({
  account_get: {
    id: "account_get",
    method: "GET",
    path: "/api/account",
    plane: "central",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["organizationId"],
    pauseExempt: true,
  },
  api_key_create: {
    id: "api_key_create",
    method: "POST",
    path: "/api/api-keys",
    plane: "central",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: [],
    queryParams: [],
    pauseExempt: false,
  },
  api_key_revoke: {
    id: "api_key_revoke",
    method: "DELETE",
    path: "/api/api-keys/{apiKeyId}",
    plane: "central",
    safeRetry: false,
    idempotency: "none",
    transport: "unary",
    pathParams: ["apiKeyId"],
    queryParams: [],
    pauseExempt: true,
  },
  api_keys_list: {
    id: "api_keys_list",
    method: "GET",
    path: "/api/api-keys",
    plane: "central",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["cursor", "limit", "workspaceId"],
    pauseExempt: false,
  },
  billing_auto_topup_policy_get: {
    id: "billing_auto_topup_policy_get",
    method: "GET",
    path: "/api/organizations/{organizationId}/billing/auto-topup-policy",
    plane: "central",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: ["organizationId"],
    queryParams: [],
    pauseExempt: true,
  },
  billing_auto_topup_policy_put: {
    id: "billing_auto_topup_policy_put",
    method: "PUT",
    path: "/api/organizations/{organizationId}/billing/auto-topup-policy",
    plane: "central",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: ["organizationId"],
    queryParams: [],
    pauseExempt: true,
  },
  billing_balance_get: {
    id: "billing_balance_get",
    method: "GET",
    path: "/api/billing/balance",
    plane: "central",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["organizationId"],
    pauseExempt: true,
  },
  billing_portal_session_create: {
    id: "billing_portal_session_create",
    method: "POST",
    path: "/api/organizations/{organizationId}/billing/portal-sessions",
    plane: "central",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: ["organizationId"],
    queryParams: [],
    pauseExempt: true,
  },
  billing_statement_download_create: {
    id: "billing_statement_download_create",
    method: "POST",
    path: "/api/organizations/{organizationId}/billing/statements/{statementId}/downloads",
    plane: "central",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: ["organizationId", "statementId"],
    queryParams: [],
    pauseExempt: true,
  },
  billing_statements_list: {
    id: "billing_statements_list",
    method: "GET",
    path: "/api/organizations/{organizationId}/billing/statements",
    plane: "central",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: ["organizationId"],
    queryParams: ["cursor", "limit"],
    pauseExempt: true,
  },
  billing_top_up_checkout_create: {
    id: "billing_top_up_checkout_create",
    method: "POST",
    path: "/api/organizations/{organizationId}/billing/top-up-checkouts",
    plane: "central",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: ["organizationId"],
    queryParams: [],
    pauseExempt: true,
  },
  dashboard_bootstrap_get: {
    id: "dashboard_bootstrap_get",
    method: "GET",
    path: "/api/bootstrap",
    plane: "central",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: [],
    pauseExempt: true,
  },
  observations_events_query: {
    id: "observations_events_query",
    method: "POST",
    path: "/api/events/query",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: [],
    pauseExempt: false,
  },
  observations_metrics_aggregate: {
    id: "observations_metrics_aggregate",
    method: "POST",
    path: "/api/metrics/aggregate",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: [],
    pauseExempt: false,
  },
  observations_traces_query: {
    id: "observations_traces_query",
    method: "POST",
    path: "/api/traces/query",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: [],
    pauseExempt: false,
  },
  organization_create: {
    id: "organization_create",
    method: "POST",
    path: "/api/organizations",
    plane: "central",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: [],
    queryParams: [],
    pauseExempt: false,
  },
  registry_files_download_create: {
    id: "registry_files_download_create",
    method: "POST",
    path: "/api/workspace/files/{name}/downloads",
    plane: "regional",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: ["name"],
    queryParams: [],
    pauseExempt: false,
  },
  registry_files_list: {
    id: "registry_files_list",
    method: "GET",
    path: "/api/workspace/files",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["cursor", "limit"],
    pauseExempt: false,
  },
  registry_instructions_list: {
    id: "registry_instructions_list",
    method: "GET",
    path: "/api/workspace/instructions",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["cursor", "limit"],
    pauseExempt: false,
  },
  registry_mcp_servers_list: {
    id: "registry_mcp_servers_list",
    method: "GET",
    path: "/api/workspace/mcp-servers",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["cursor", "limit"],
    pauseExempt: false,
  },
  registry_skills_list: {
    id: "registry_skills_list",
    method: "GET",
    path: "/api/workspace/skills",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["cursor", "limit"],
    pauseExempt: false,
  },
  registry_tools_list: {
    id: "registry_tools_list",
    method: "GET",
    path: "/api/workspace/tools",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["cursor", "limit"],
    pauseExempt: false,
  },
  secret_delete: {
    id: "secret_delete",
    method: "DELETE",
    path: "/api/workspace/secrets/{name}",
    plane: "regional",
    safeRetry: false,
    idempotency: "none",
    transport: "unary",
    pathParams: ["name"],
    queryParams: [],
    pauseExempt: true,
  },
  secret_put: {
    id: "secret_put",
    method: "PUT",
    path: "/api/workspace/secrets/{name}",
    plane: "regional",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: ["name"],
    queryParams: [],
    pauseExempt: false,
  },
  secret_revoke: {
    id: "secret_revoke",
    method: "POST",
    path: "/api/workspace/secrets/{name}/revocations",
    plane: "regional",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: ["name"],
    queryParams: [],
    pauseExempt: true,
  },
  secrets_list: {
    id: "secrets_list",
    method: "GET",
    path: "/api/workspace/secrets",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["cursor", "limit"],
    pauseExempt: false,
  },
  session_approval_respond: {
    id: "session_approval_respond",
    method: "POST",
    path: "/api/sessions/{sessionId}/approvals/{approvalId}/responses",
    plane: "regional",
    safeRetry: false,
    idempotency: "none",
    transport: "unary",
    pathParams: ["sessionId", "approvalId"],
    queryParams: [],
    pauseExempt: false,
  },
  session_approvals_list: {
    id: "session_approvals_list",
    method: "GET",
    path: "/api/sessions/{sessionId}/approvals",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: ["sessionId"],
    queryParams: ["cursor", "limit"],
    pauseExempt: false,
  },
  session_create: {
    id: "session_create",
    method: "POST",
    path: "/api/sessions",
    plane: "regional",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: [],
    queryParams: [],
    pauseExempt: false,
  },
  session_files_persisted_download_create: {
    id: "session_files_persisted_download_create",
    method: "POST",
    path: "/api/sessions/{sessionId}/files/persisted/downloads",
    plane: "regional",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: ["sessionId"],
    queryParams: [],
    pauseExempt: false,
  },
  session_files_persisted_list: {
    id: "session_files_persisted_list",
    method: "POST",
    path: "/api/sessions/{sessionId}/files/persisted/list",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: ["sessionId"],
    queryParams: [],
    pauseExempt: false,
  },
  session_get: {
    id: "session_get",
    method: "GET",
    path: "/api/sessions/{sessionId}",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: ["sessionId"],
    queryParams: [],
    pauseExempt: false,
  },
  session_observations_events_query: {
    id: "session_observations_events_query",
    method: "POST",
    path: "/api/sessions/{sessionId}/events/query",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: ["sessionId"],
    queryParams: [],
    pauseExempt: false,
  },
  session_observations_trace_get: {
    id: "session_observations_trace_get",
    method: "GET",
    path: "/api/sessions/{sessionId}/traces/{traceId}",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: ["sessionId", "traceId"],
    queryParams: [],
    pauseExempt: false,
  },
  session_runs_list: {
    id: "session_runs_list",
    method: "GET",
    path: "/api/sessions/{sessionId}/runs",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: ["sessionId"],
    queryParams: ["cursor", "limit"],
    pauseExempt: false,
  },
  sessions_list: {
    id: "sessions_list",
    method: "GET",
    path: "/api/sessions",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["cursor", "limit", "status"],
    pauseExempt: false,
  },
  telemetry_gaps_query: {
    id: "telemetry_gaps_query",
    method: "POST",
    path: "/api/telemetry/gaps/query",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: [],
    pauseExempt: false,
  },
  usage_query: {
    id: "usage_query",
    method: "POST",
    path: "/api/billing/usage/query",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["workspaceId"],
    pauseExempt: true,
  },
  workspace_create: {
    id: "workspace_create",
    method: "POST",
    path: "/api/workspaces",
    plane: "central",
    safeRetry: false,
    idempotency: "idempotency_key",
    transport: "unary",
    pathParams: [],
    queryParams: [],
    pauseExempt: false,
  },
  workspace_get: {
    id: "workspace_get",
    method: "GET",
    path: "/api/workspaces/{workspaceId}",
    plane: "central",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: ["workspaceId"],
    queryParams: [],
    pauseExempt: false,
  },
  workspace_limits_list: {
    id: "workspace_limits_list",
    method: "GET",
    path: "/api/workspace/limits",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
    transport: "unary",
    pathParams: [],
    queryParams: ["cursor", "limit"],
    pauseExempt: true,
  },
});

export type ErrorClass =
  | "auth"
  | "not_found"
  | "conflict"
  | "precondition"
  | "validation"
  | "quota"
  | "state"
  | "unavailable"
  | "internal";

export type AexErrorCode = string;

export const ERROR_METADATA: Readonly<Record<string, { readonly class: ErrorClass }>> =
  Object.freeze({
    account_paused: { class: "state" },
    account_state_unavailable: { class: "unavailable" },
    approval_already_resolved: { class: "conflict" },
    approval_binding_changed: { class: "conflict" },
    approval_not_found: { class: "not_found" },
    authentication_unavailable: { class: "unavailable" },
    authorization_pending: { class: "state" },
    content_missing: { class: "state" },
    deletion_in_progress: { class: "conflict" },
    download_grant_expired: { class: "not_found" },
    export_expired: { class: "not_found" },
    export_not_found: { class: "not_found" },
    export_not_ready: { class: "state" },
    export_revoked: { class: "not_found" },
    file_not_found: { class: "not_found" },
    forbidden: { class: "auth" },
    gone: { class: "not_found" },
    idempotency_conflict: { class: "conflict" },
    insufficient_scope: { class: "auth" },
    internal_error: { class: "internal" },
    invalid_auto_topup_policy: { class: "validation" },
    invalid_cursor: { class: "validation" },
    invalid_file_selection: { class: "validation" },
    invalid_metric_aggregation: { class: "validation" },
    invalid_network_policy: { class: "validation" },
    invalid_query: { class: "validation" },
    invalid_range: { class: "validation" },
    invalid_request: { class: "validation" },
    invalid_telemetry: { class: "validation" },
    limit_exceeded: { class: "quota" },
    malformed_token: { class: "auth" },
    not_found: { class: "not_found" },
    observability_unavailable: { class: "unavailable" },
    operation_idempotency_conflict: { class: "conflict" },
    operation_not_cancelable: { class: "state" },
    package_artifact_unavailable: { class: "state" },
    package_integrity_mismatch: { class: "state" },
    package_resolution_failed: { class: "state" },
    payload_too_large: { class: "validation" },
    payment_method_required: { class: "state" },
    precondition_failed: { class: "precondition" },
    provider_credential_not_found: { class: "not_found" },
    provider_credential_revoked: { class: "state" },
    rate_limited: { class: "quota" },
    session_deleted: { class: "not_found" },
    session_deleting: { class: "state" },
    session_not_idle: { class: "state" },
    slow_down: { class: "quota" },
    telemetry_incomplete: { class: "state" },
    telemetry_payload_too_large: { class: "validation" },
    telemetry_quota_exceeded: { class: "quota" },
    token_expired: { class: "auth" },
    token_invalid: { class: "auth" },
    token_revoked: { class: "auth" },
    unauthenticated: { class: "auth" },
    unknown_model: { class: "validation" },
    unknown_provider: { class: "validation" },
    unqualified_provider_model: { class: "state" },
    unsupported_export_signal: { class: "validation" },
    unsupported_package_ecosystem: { class: "validation" },
    upstream_error: { class: "unavailable" },
    workspace_activation_required: { class: "state" },
    workspace_not_live: { class: "state" },
    wrong_workspace_region: { class: "conflict" },
  });
