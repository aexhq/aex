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
  // The human end of the CLI device flow. This is the one operation a browser
  // must reach for `aex auth login` to complete at all: nothing else in the
  // product moves a device authorization off `pending`.
  "device_decision_create",
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
  // Session conversation and live files.
  "sessions_list",
  "session_get",
  "session_messages_list",
  "session_files_live_list",
  // Workspace resources.
  "provider_credentials_list",
  "provider_credential_register",
  "provider_credential_revoke",
  "registry_files_list",
  "registry_files_download_create",
  "workspace_limits_list",
  // Usage.
  "usage_query",
] as const satisfies readonly RouteId[];

export type DashboardRouteId = (typeof DASHBOARD_ROUTES)[number];

export function authorizeDashboardRoute(routeId: string): RouteDescriptor | null {
  if (!(DASHBOARD_ROUTES as readonly string[]).includes(routeId)) return null;
  return ROUTES[routeId as RouteId] ?? null;
}
