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
  // API keys.
  "api_keys_list",
  "api_key_create",
  "api_key_revoke",
  // Essential prepaid billing.
  "billing_balance_get",
  "billing_payment_method_delete",
  "billing_payment_method_session_create",
  "billing_payment_methods_list",
  "billing_top_up_checkout_create",
  "billing_transactions_list",
  "billing_usage_get",
  // Latest workspace files.
  "registry_files_delete",
  "registry_files_download_create",
  "registry_files_get",
  "registry_files_list",
  "registry_files_put",
  "upload_create",
  "upload_complete",
  // Session lifecycle, conversation and retained/live observability.
  "sessions_list",
  "session_create",
  "session_get",
  "session_cancel",
  "session_delete",
  "session_message_send",
  "session_messages_list",
  "session_messages_stream",
  "session_telemetry_download_create",
  "session_telemetry_replay",
  "session_telemetry_stream",
  "session_terminate",
] as const satisfies readonly RouteId[];

export type DashboardRouteId = (typeof DASHBOARD_ROUTES)[number];

export function authorizeDashboardRoute(routeId: string): RouteDescriptor | null {
  if (!(DASHBOARD_ROUTES as readonly string[]).includes(routeId)) return null;
  return ROUTES[routeId as RouteId] ?? null;
}
