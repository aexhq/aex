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

function registryRoutes(
  name: "files" | "skills" | "tools" | "instructions" | "mcpServers",
  path: "files" | "skills" | "tools" | "instructions" | "mcp-servers"
): readonly AuthenticatedApiRouteDescriptor[] {
  const escaped = path.replace("-", "\\-");
  const routes: AuthenticatedApiRouteDescriptor[] = [
    regional(`registry.${name}.list`, "GET", new RegExp(`^/workspace/${escaped}$`), `/workspace/${path}`, "resources:read"),
    regional(`registry.${name}.get`, "GET", new RegExp(`^/workspace/${escaped}/[^/]+$`), `/workspace/${path}/example`, "resources:read"),
    regional(
      `registry.${name}.put`,
      "PUT",
      new RegExp(`^/workspace/${escaped}/[^/]+$`),
      `/workspace/${path}/example`,
      "resources:write",
      "idempotency-key"
    ),
    regional(`registry.${name}.delete`, "DELETE", new RegExp(`^/workspace/${escaped}/[^/]+$`), `/workspace/${path}/example`, "resources:write")
  ];
  if (name === "files") {
    routes.push(regional(
      "registry.files.download",
      "POST",
      /^\/workspace\/files\/[^/]+\/downloads$/,
      "/workspace/files/example/downloads",
      "resources:read",
      "idempotency-key"
    ));
  }
  return routes;
}

function observationRoutes(
  signal: "events" | "logs" | "spans" | "metrics" | "traces" | "telemetry",
  sessionScoped: boolean
): readonly AuthenticatedApiRouteDescriptor[] {
  const prefix = sessionScoped ? `/sessions/[^/]+/${signal}` : `/${signal}`;
  const samplePrefix = sessionScoped ? `/sessions/ses_1/${signal}` : `/${signal}`;
  const namePrefix = sessionScoped ? `session.${signal}` : signal;
  return (["query", "stream", "listen"] as const).map((action) =>
    regional(
      `${namePrefix}.${action}`,
      "POST",
      new RegExp(`^${prefix}/${action}$`),
      `${samplePrefix}/${action}`,
      "telemetry:read"
    )
  );
}

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
  bootstrap(
    "billing.statements.download",
    "POST",
    /^\/organizations\/[^/]+\/billing\/statements\/[^/]+\/downloads$/,
    "/organizations/org_1/billing/statements/stm_1/downloads",
    "billing:read",
    "idempotency-key"
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
    "workspace.limits.list",
    "GET",
    /^\/workspace\/limits$/,
    "/workspace/limits",
    "workspace:read"
  ),
  regional(
    "workspace.limits.get",
    "GET",
    /^\/workspace\/limits\/[^/]+$/,
    "/workspace/limits/query.page",
    "workspace:read"
  ),
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
  ),
  regional(
    "files.persisted.list",
    "POST",
    /^\/sessions\/[^/]+\/files\/persisted\/list$/,
    "/sessions/ses_1/files/persisted/list",
    "files:read"
  ),
  regional(
    "files.persisted.stat",
    "POST",
    /^\/sessions\/[^/]+\/files\/persisted\/stat$/,
    "/sessions/ses_1/files/persisted/stat",
    "files:read"
  ),
  regional(
    "files.persisted.download",
    "POST",
    /^\/sessions\/[^/]+\/files\/persisted\/downloads$/,
    "/sessions/ses_1/files/persisted/downloads",
    "files:read",
    "idempotency-key"
  ),
  regional(
    "files.live.list",
    "POST",
    /^\/sessions\/[^/]+\/files\/live\/list$/,
    "/sessions/ses_1/files/live/list",
    "files:live"
  ),
  regional(
    "files.live.stat",
    "POST",
    /^\/sessions\/[^/]+\/files\/live\/stat$/,
    "/sessions/ses_1/files/live/stat",
    "files:live"
  ),
  regional(
    "files.live.download",
    "POST",
    /^\/sessions\/[^/]+\/files\/live\/downloads$/,
    "/sessions/ses_1/files/live/downloads",
    "files:live",
    "idempotency-key"
  ),
  ...registryRoutes("files", "files"),
  ...registryRoutes("skills", "skills"),
  ...registryRoutes("tools", "tools"),
  ...registryRoutes("instructions", "instructions"),
  ...registryRoutes("mcpServers", "mcp-servers"),
  regional(
    "uploads.create",
    "POST",
    /^\/workspace\/uploads$/,
    "/workspace/uploads",
    "resources:write",
    "idempotency-key"
  ),
  regional(
    "uploads.parts",
    "POST",
    /^\/workspace\/uploads\/[^/]+\/parts$/,
    "/workspace/uploads/upl_1/parts",
    "resources:write"
  ),
  regional(
    "uploads.complete",
    "POST",
    /^\/workspace\/uploads\/[^/]+\/completion$/,
    "/workspace/uploads/upl_1/completion",
    "resources:write",
    "idempotency-key"
  ),
  regional(
    "uploads.abort",
    "DELETE",
    /^\/workspace\/uploads\/[^/]+$/,
    "/workspace/uploads/upl_1",
    "resources:write"
  ),
  regional("secrets.list", "GET", /^\/workspace\/secrets$/, "/workspace/secrets", "secrets:read"),
  regional(
    "secrets.get",
    "GET",
    /^\/workspace\/secrets\/[^/]+$/,
    "/workspace/secrets/GITHUB_TOKEN",
    "secrets:read"
  ),
  regional(
    "secrets.put",
    "PUT",
    /^\/workspace\/secrets\/[^/]+$/,
    "/workspace/secrets/GITHUB_TOKEN",
    "secrets:write",
    "idempotency-key"
  ),
  regional(
    "secrets.delete",
    "DELETE",
    /^\/workspace\/secrets\/[^/]+$/,
    "/workspace/secrets/GITHUB_TOKEN",
    "secrets:write"
  ),
  regional(
    "secrets.revoke",
    "POST",
    /^\/workspace\/secrets\/[^/]+\/revocations$/,
    "/workspace/secrets/GITHUB_TOKEN/revocations",
    "secrets:revoke",
    "idempotency-key"
  ),
  regional(
    "approvals.list",
    "GET",
    /^\/sessions\/[^/]+\/approvals$/,
    "/sessions/ses_1/approvals",
    "sessions:read"
  ),
  regional(
    "approvals.get",
    "GET",
    /^\/sessions\/[^/]+\/approvals\/[^/]+$/,
    "/sessions/ses_1/approvals/apr_1",
    "sessions:read"
  ),
  regional(
    "approvals.respond",
    "POST",
    /^\/sessions\/[^/]+\/approvals\/[^/]+\/responses$/,
    "/sessions/ses_1/approvals/apr_1/responses",
    "sessions:write"
  ),
  regional(
    "billing.usage.query",
    "POST",
    /^\/billing\/usage\/query$/,
    "/billing/usage/query",
    "billing:read"
  ),
  ...observationRoutes("events", false),
  ...observationRoutes("logs", false),
  ...observationRoutes("spans", false),
  ...observationRoutes("metrics", false),
  ...observationRoutes("traces", false),
  ...observationRoutes("telemetry", false),
  ...observationRoutes("events", true),
  ...observationRoutes("logs", true),
  ...observationRoutes("spans", true),
  ...observationRoutes("metrics", true),
  ...observationRoutes("traces", true),
  ...observationRoutes("telemetry", true),
  regional(
    "metrics.aggregate",
    "POST",
    /^\/metrics\/aggregate$/,
    "/metrics/aggregate",
    "telemetry:read"
  ),
  regional(
    "session.metrics.aggregate",
    "POST",
    /^\/sessions\/[^/]+\/metrics\/aggregate$/,
    "/sessions/ses_1/metrics/aggregate",
    "telemetry:read"
  ),
  regional(
    "session.traces.get",
    "GET",
    /^\/sessions\/[^/]+\/traces\/[0-9a-f]{32}$/,
    "/sessions/ses_1/traces/0123456789abcdef0123456789abcdef",
    "telemetry:read"
  ),
  regional(
    "telemetry.otlp.logs",
    "POST",
    /^\/telemetry\/otlp\/v1\/logs$/,
    "/telemetry/otlp/v1/logs",
    "telemetry:write",
    "idempotency-key"
  ),
  regional(
    "telemetry.otlp.traces",
    "POST",
    /^\/telemetry\/otlp\/v1\/traces$/,
    "/telemetry/otlp/v1/traces",
    "telemetry:write",
    "idempotency-key"
  ),
  regional(
    "telemetry.otlp.metrics",
    "POST",
    /^\/telemetry\/otlp\/v1\/metrics$/,
    "/telemetry/otlp/v1/metrics",
    "telemetry:write",
    "idempotency-key"
  ),
  regional(
    "telemetry.gaps.query",
    "POST",
    /^\/telemetry\/gaps\/query$/,
    "/telemetry/gaps/query",
    "telemetry:read"
  ),
  regional(
    "telemetry.gaps.get",
    "GET",
    /^\/telemetry\/gaps\/[^/]+$/,
    "/telemetry/gaps/gap_1",
    "telemetry:read"
  ),
  regional(
    "session.telemetry.gaps.query",
    "POST",
    /^\/sessions\/[^/]+\/telemetry\/gaps\/query$/,
    "/sessions/ses_1/telemetry/gaps/query",
    "telemetry:read"
  ),
  regional(
    "session.telemetry.gaps.get",
    "GET",
    /^\/sessions\/[^/]+\/telemetry\/gaps\/[^/]+$/,
    "/sessions/ses_1/telemetry/gaps/gap_1",
    "telemetry:read"
  ),
  regional(
    "telemetry.exports.create",
    "POST",
    /^\/telemetry\/exports$/,
    "/telemetry/exports",
    "telemetry:read",
    "operation-id"
  ),
  regional(
    "telemetry.exports.get",
    "GET",
    /^\/telemetry\/exports\/[^/]+$/,
    "/telemetry/exports/exp_1",
    "telemetry:read"
  ),
  regional(
    "telemetry.exports.download",
    "POST",
    /^\/telemetry\/exports\/[^/]+\/downloads$/,
    "/telemetry/exports/exp_1/downloads",
    "telemetry:read",
    "idempotency-key"
  ),
  regional(
    "telemetry.exports.revoke",
    "POST",
    /^\/telemetry\/exports\/[^/]+\/revocations$/,
    "/telemetry/exports/exp_1/revocations",
    "telemetry:read",
    "idempotency-key"
  ),
  regional(
    "session.telemetry.exports.create",
    "POST",
    /^\/sessions\/[^/]+\/telemetry\/exports$/,
    "/sessions/ses_1/telemetry/exports",
    "telemetry:read",
    "operation-id"
  ),
  regional(
    "session.telemetry.exports.get",
    "GET",
    /^\/sessions\/[^/]+\/telemetry\/exports\/[^/]+$/,
    "/sessions/ses_1/telemetry/exports/exp_1",
    "telemetry:read"
  ),
  regional(
    "session.telemetry.exports.download",
    "POST",
    /^\/sessions\/[^/]+\/telemetry\/exports\/[^/]+\/downloads$/,
    "/sessions/ses_1/telemetry/exports/exp_1/downloads",
    "telemetry:read",
    "idempotency-key"
  ),
  regional(
    "session.telemetry.exports.revoke",
    "POST",
    /^\/sessions\/[^/]+\/telemetry\/exports\/[^/]+\/revocations$/,
    "/sessions/ses_1/telemetry/exports/exp_1/revocations",
    "telemetry:read",
    "idempotency-key"
  )
] as const satisfies readonly AuthenticatedApiRouteDescriptor[];
