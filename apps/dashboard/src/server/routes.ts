import { ROUTES, type RouteDescriptor, type RouteId } from "@aexhq/sdk";

/**
 * The whole data path of the dashboard.
 *
 * Every surface the dashboard renders is one of these operations. A request whose
 * route id is not a member is refused before the upstream call and before the
 * browser credential is attached. Nothing here is an operator route, and nothing
 * here is a write the product cannot already express — the dashboard invents no
 * capability.
 *
 * Adding a row is a product decision: it widens what a stolen browser session can
 * reach. `test/routes.test.ts` pins the list to `ROUTES`, which is itself pinned to
 * `api/generated/registries/routes.json`.
 */
export const DASHBOARD_ROUTES = [
  // Shell and account.
  "account_get",
  "organization_create",
  "workspace_create",
  // API keys.
  "api_keys_list",
  "api_key_create",
  "api_key_revoke",
  // Billing, organization-scoped.
  "billing_balance_get",
  "billing_statements_list",
  "billing_statement_download_create",
  "billing_auto_topup_policy_get",
  "billing_auto_topup_policy_put",
  "billing_top_up_checkout_create",
  "billing_portal_session_create",
  // Sessions and runs.
  "sessions_list",
  "session_get",
  "session_runs_list",
  "session_approvals_list",
  "session_approval_respond",
  "session_files_persisted_list",
  "session_files_persisted_download_create",
  // Observability.
  "observations_events_query",
  "observations_traces_query",
  // `observations_metrics_aggregate` is deliberately absent: the contract has no
  // metric-name discovery operation, so no metric picker can be offered and a
  // free-text metric box is a worse answer than no panel. Recorded, not mocked.
  "session_observations_events_query",
  "session_observations_trace_get",
  "telemetry_gaps_query",
  // Workspace resources.
  "secrets_list",
  "secret_put",
  "secret_delete",
  "secret_revoke",
  "registry_files_list",
  "registry_files_download_create",
  "registry_tools_list",
  "registry_skills_list",
  "registry_mcp_servers_list",
  "registry_instructions_list",
  "workspace_limits_list",
  // Usage.
  "usage_query",
] as const satisfies readonly RouteId[];

export type DashboardRouteId = (typeof DASHBOARD_ROUTES)[number];

export function authorizeDashboardRoute(routeId: string): RouteDescriptor | null {
  if (!(DASHBOARD_ROUTES as readonly string[]).includes(routeId)) return null;
  return ROUTES[routeId as RouteId] ?? null;
}
