/**
 * The prelaunch v1 authenticated route authorities.
 *
 * Bootstrap and regional hosts use different principals and are deliberately
 * separate tables. The hosted dispatchers and OpenAPI generator consume these
 * declarations directly; legacy runtime/checkpoint/webhook routes have no
 * compatibility entries.
 */

export type RequiredApiScope = string;
export type ApiPlane = "bootstrap" | "regional";
export type RouteIdempotency = "none" | "idempotency-key" | "operation-id";

export type AuthenticatedApiRouteDescriptor = {
  readonly plane: ApiPlane;
  readonly name: string;
  readonly method: "GET" | "POST" | "PUT" | "DELETE";
  readonly pattern: RegExp;
  readonly samplePath: string;
  readonly requiredScope: RequiredApiScope | null;
  /**
   * `operation-id` means the required `Aex-Operation-Id` header.
   * `idempotency-key` means the required `Idempotency-Key` header.
   */
  readonly idempotency: RouteIdempotency;
};

function route(
  plane: ApiPlane,
  name: string,
  method: AuthenticatedApiRouteDescriptor["method"],
  pattern: RegExp,
  samplePath: string,
  requiredScope: RequiredApiScope | null,
  idempotency: RouteIdempotency = "none"
): AuthenticatedApiRouteDescriptor {
  return { plane, name, method, pattern, samplePath, requiredScope, idempotency };
}

const bootstrap = (
  name: string,
  method: AuthenticatedApiRouteDescriptor["method"],
  pattern: RegExp,
  samplePath: string,
  requiredScope: RequiredApiScope | null,
  idempotency: RouteIdempotency = "none"
) => route("bootstrap", name, method, pattern, samplePath, requiredScope, idempotency);

const regional = (
  name: string,
  method: AuthenticatedApiRouteDescriptor["method"],
  pattern: RegExp,
  samplePath: string,
  requiredScope: RequiredApiScope | null,
  idempotency: RouteIdempotency = "none"
) => route("regional", name, method, pattern, samplePath, requiredScope, idempotency);

export const BOOTSTRAP_API_ROUTE_DESCRIPTORS = [
  bootstrap("account.get", "GET", /^\/account$/, "/account", "account:read"),
  bootstrap("organizations.list", "GET", /^\/organizations$/, "/organizations", "organizations:read"),
  bootstrap(
    "organizations.create",
    "POST",
    /^\/organizations$/,
    "/organizations",
    "organizations:write",
    "idempotency-key"
  ),
  bootstrap(
    "organizations.get",
    "GET",
    /^\/organizations\/[^/]+$/,
    "/organizations/org_1",
    "organizations:read"
  ),
  bootstrap(
    "memberships.list",
    "GET",
    /^\/organizations\/[^/]+\/memberships$/,
    "/organizations/org_1/memberships",
    "memberships:read"
  ),
  bootstrap(
    "invitations.create",
    "POST",
    /^\/organizations\/[^/]+\/invitations$/,
    "/organizations/org_1/invitations",
    "memberships:write",
    "idempotency-key"
  ),
  bootstrap("workspaces.list", "GET", /^\/workspaces$/, "/workspaces", "workspaces:read"),
  bootstrap(
    "workspaces.create",
    "POST",
    /^\/workspaces$/,
    "/workspaces",
    "workspaces:write",
    "idempotency-key"
  ),
  bootstrap(
    "workspaces.get",
    "GET",
    /^\/workspaces\/[^/]+$/,
    "/workspaces/wsp_1",
    "workspaces:read"
  ),
  bootstrap(
    "workspaces.delete",
    "POST",
    /^\/workspaces\/[^/]+\/deletions$/,
    "/workspaces/wsp_1/deletions",
    "workspaces:delete",
    "operation-id"
  ),
  bootstrap("apiKeys.list", "GET", /^\/api-keys$/, "/api-keys", "api_keys:read"),
  bootstrap(
    "apiKeys.create",
    "POST",
    /^\/api-keys$/,
    "/api-keys",
    "api_keys:write",
    "idempotency-key"
  ),
  bootstrap(
    "apiKeys.delete",
    "DELETE",
    /^\/api-keys\/[^/]+$/,
    "/api-keys/key_1",
    "api_keys:write"
  ),
  bootstrap("billing.balance", "GET", /^\/billing\/balance$/, "/billing/balance", "billing:read"),
  bootstrap(
    "billing.topUpCheckout",
    "POST",
    /^\/organizations\/[^/]+\/billing\/top-up-checkouts$/,
    "/organizations/org_1/billing/top-up-checkouts",
    "billing:write",
    "idempotency-key"
  ),
  bootstrap(
    "billing.portalSession",
    "POST",
    /^\/organizations\/[^/]+\/billing\/portal-sessions$/,
    "/organizations/org_1/billing/portal-sessions",
    "billing:write",
    "idempotency-key"
  ),
  bootstrap(
    "billing.autoTopup.get",
    "GET",
    /^\/organizations\/[^/]+\/billing\/auto-topup-policy$/,
    "/organizations/org_1/billing/auto-topup-policy",
    "billing:read"
  ),
  bootstrap(
    "billing.autoTopup.put",
    "PUT",
    /^\/organizations\/[^/]+\/billing\/auto-topup-policy$/,
    "/organizations/org_1/billing/auto-topup-policy",
    "billing:write",
    "idempotency-key"
  ),
  bootstrap(
    "billing.statements.list",
    "GET",
    /^\/organizations\/[^/]+\/billing\/statements$/,
    "/organizations/org_1/billing/statements",
    "billing:read"
  ),
  bootstrap(
    "billing.statements.get",
    "GET",
    /^\/organizations\/[^/]+\/billing\/statements\/[^/]+$/,
    "/organizations/org_1/billing/statements/stm_1",
    "billing:read"
  ),
  bootstrap("operations.list", "GET", /^\/operations$/, "/operations", "operations:read"),
  bootstrap("operations.get", "GET", /^\/operations\/[^/]+$/, "/operations/op_1", null),
  bootstrap(
    "operations.cancel",
    "POST",
    /^\/operations\/[^/]+\/cancellations$/,
    "/operations/op_1/cancellations",
    "operations:write"
  )
] as const satisfies readonly AuthenticatedApiRouteDescriptor[];

export const REGIONAL_API_ROUTE_DESCRIPTORS = [
  regional("workspace.get", "GET", /^\/workspace$/, "/workspace", "workspace:read"),
  regional(
    "sessions.create",
    "POST",
    /^\/sessions$/,
    "/sessions",
    "sessions:write",
    "idempotency-key"
  ),
  regional("sessions.list", "GET", /^\/sessions$/, "/sessions", "sessions:read"),
  regional("sessions.get", "GET", /^\/sessions\/[^/]+$/, "/sessions/ses_1", "sessions:read"),
  regional(
    "messages.list",
    "GET",
    /^\/sessions\/[^/]+\/messages$/,
    "/sessions/ses_1/messages",
    "sessions:read"
  ),
  regional(
    "messages.send",
    "POST",
    /^\/sessions\/[^/]+\/messages$/,
    "/sessions/ses_1/messages",
    "sessions:write",
    "idempotency-key"
  ),
  regional(
    "runs.list",
    "GET",
    /^\/sessions\/[^/]+\/runs$/,
    "/sessions/ses_1/runs",
    "sessions:read"
  ),
  regional(
    "runs.get",
    "GET",
    /^\/sessions\/[^/]+\/runs\/[^/]+$/,
    "/sessions/ses_1/runs/run_1",
    "sessions:read"
  ),
  regional(
    "sessions.stop",
    "POST",
    /^\/sessions\/[^/]+\/stops$/,
    "/sessions/ses_1/stops",
    "sessions:write",
    "operation-id"
  ),
  regional(
    "sessions.persist",
    "POST",
    /^\/sessions\/[^/]+\/persists$/,
    "/sessions/ses_1/persists",
    "files:write",
    "operation-id"
  ),
  regional(
    "sessions.fork",
    "POST",
    /^\/sessions\/[^/]+\/forks$/,
    "/sessions/ses_1/forks",
    "sessions:write",
    "operation-id"
  ),
  regional(
    "sessions.workspace.discard",
    "POST",
    /^\/sessions\/[^/]+\/workspace\/discards$/,
    "/sessions/ses_1/workspace/discards",
    "sessions:write",
    "operation-id"
  ),
  regional(
    "sessions.credentials.rebind",
    "POST",
    /^\/sessions\/[^/]+\/credential-rebinds$/,
    "/sessions/ses_1/credential-rebinds",
    "secrets:write",
    "operation-id"
  ),
  regional(
    "sessions.delete",
    "POST",
    /^\/sessions\/[^/]+\/deletions$/,
    "/sessions/ses_1/deletions",
    "sessions:delete",
    "operation-id"
  ),
  regional("operations.list", "GET", /^\/operations$/, "/operations", "operations:read"),
  regional(
    "operations.get",
    "GET",
    /^\/operations\/[^/]+$/,
    "/operations/op_1",
    "operations:read"
  ),
  regional(
    "operations.cancel",
    "POST",
    /^\/operations\/[^/]+\/cancellations$/,
    "/operations/op_1/cancellations",
    "operations:write"
  )
] as const satisfies readonly AuthenticatedApiRouteDescriptor[];
