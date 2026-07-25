/**
 * The authenticated data-plane HTTP route table.
 *
 * This is the single machine-readable description of the data-plane surface:
 * the platform api lambda dispatches and scope-gates from it, and the OpenAPI
 * generator in this repo reads the same table. It is declared here exactly
 * once so the two can never drift.
 *
 * Declaration-only and dependency-free: matching, scope lookup and route
 * classification stay with the server that dispatches them.
 */

export type RequiredApiScope = string;

export type AuthenticatedApiRouteDescriptor = {
  readonly name: string;
  readonly method: string;
  readonly pattern: RegExp;
  readonly samplePath: string;
  readonly requiredScope: RequiredApiScope | null;
};

const route = (
  name: string,
  method: string,
  pattern: RegExp,
  samplePath: string,
  requiredScope: RequiredApiScope | null,
): AuthenticatedApiRouteDescriptor => ({
  name,
  method,
  pattern,
  samplePath,
  requiredScope,
});

export const AUTHENTICATED_API_ROUTE_DESCRIPTORS: readonly AuthenticatedApiRouteDescriptor[] = [
  // Valid-token canary.
  route("whoami", "GET", /^\/whoami$/, "/whoami", null),
  // Private runtime liveness of the exact writer baton. This is writer-token
  // only at dispatch; it is not a bearer/public SDK surface.
  route("runtime.writerAuthority", "GET", /^\/runtime\/writer-authority$/, "/runtime/writer-authority", null),
  // (storage-model WS2) The credential-less container's DDB-decided journal commit channel: the api
  // runs the fenced control-item write on its behalf. Writer-token only (same channel as
  // writer-authority); the WS1 writerEpoch fence is applied at dispatch before this route runs.
  route("runtime.journalCommit", "POST", /^\/runtime\/journal\/commit$/, "/runtime/journal/commit", null),
  // Sessions.
  route("sessions.create", "POST", /^\/sessions$/, "/sessions", "sessions:write"),
  route("sessions.list", "GET", /^\/sessions$/, "/sessions", "sessions:read"),
  route("sessions.listMessages", "GET", /^\/sessions\/[^/]+\/messages$/, "/sessions/sess_1/messages", "sessions:read"),
  route("sessions.sendMessage", "POST", /^\/sessions\/[^/]+\/messages$/, "/sessions/sess_1/messages", "sessions:write"),
  route("sessions.suspend", "POST", /^\/sessions\/[^/]+\/suspend$/, "/sessions/sess_1/suspend", "sessions:write"),
  route("sessions.cancel", "POST", /^\/sessions\/[^/]+\/cancel$/, "/sessions/sess_1/cancel", "sessions:cancel"),
  route("sessions.resume", "POST", /^\/sessions\/[^/]+\/resume$/, "/sessions/sess_1/resume", "sessions:write"),
  route("sessions.requestApproval", "POST", /^\/sessions\/[^/]+\/request-approval$/, "/sessions/sess_1/request-approval", "sessions:write"),
  route("sessions.approve", "POST", /^\/sessions\/[^/]+\/approve$/, "/sessions/sess_1/approve", "sessions:write"),
  route("sessions.deny", "POST", /^\/sessions\/[^/]+\/deny$/, "/sessions/sess_1/deny", "sessions:write"),
  route("sessions.delete", "DELETE", /^\/sessions\/[^/]+$/, "/sessions/sess_1", "sessions:delete"),
  route("sessions.eventsTicket", "POST", /^\/sessions\/[^/]+\/events\/ticket$/, "/sessions/sess_1/events/ticket", "sessions:read"),
  route("sessions.listEvents", "GET", /^\/sessions\/[^/]+\/events$/, "/sessions/sess_1/events", "sessions:read"),
  route("sessions.otel", "GET", /^\/sessions\/[^/]+\/otel$/, "/sessions/sess_1/otel", "sessions:read"),
  route("sessions.listChildren", "GET", /^\/sessions\/[^/]+\/children$/, "/sessions/sess_1/children", null),
  route("sessions.childResult", "GET", /^\/sessions\/[^/]+\/result$/, "/sessions/sess_1/result", null),
  route("sessions.eventArchiveLink", "POST", /^\/sessions\/[^/]+\/events\/link$/, "/sessions/sess_1/events/link", "sessions:read"),
  route("sessions.downloadFile", "GET", /^\/sessions\/[^/]+\/files\/[^/]+\/download$/, "/sessions/sess_1/files/file_1/download", "files:read"),
  route("sessions.fileLink", "POST", /^\/sessions\/[^/]+\/files\/[^/]+\/link$/, "/sessions/sess_1/files/file_1/link", "files:read"),
  route("sessions.listFiles", "GET", /^\/sessions\/[^/]+\/files$/, "/sessions/sess_1/files", "files:read"),
  route("sessions.listWebhookDeliveries", "GET", /^\/sessions\/[^/]+\/webhook-deliveries$/, "/sessions/sess_1/webhook-deliveries", "sessions:read"),
  route("sessions.redeliverWebhook", "POST", /^\/sessions\/[^/]+\/webhook-deliveries\/[^/]+\/redeliver$/, "/sessions/sess_1/webhook-deliveries/del_1/redeliver", "sessions:write"),
  route("sessions.archiveInternal", "GET", /^\/internal\/sessions\/[^/]+\/archive$/, "/internal/sessions/sess_1/archive", "files:read"),
  route("sessions.get", "GET", /^\/sessions\/[^/]+$/, "/sessions/sess_1", "sessions:read"),
  route("sessions.finalize", "POST", /^\/sessions\/[^/]+\/finalize$/, "/sessions/ses_1/finalize", "sessions:write"),
  // Immutable content-addressed bytes backing every workspace resource family.
  route("assets.presign", "POST", /^\/assets\/presign$/, "/assets/presign", "assets:write"),
  route("assets.finalize", "POST", /^\/assets\/finalize$/, "/assets/finalize", "assets:write"),
  route("assets.mpuPresignParts", "POST", /^\/assets\/mpu\/presign-parts$/, "/assets/mpu/presign-parts", "assets:write"),
  route("assets.mpuAbort", "POST", /^\/assets\/mpu\/abort$/, "/assets/mpu/abort", "assets:write"),
  route("assets.delete", "DELETE", /^\/assets\/[^/]+$/, "/assets/asset_1", "assets:delete"),
  // Versioned workspace resources. The raw bytes stay in /assets; these routes
  // publish and address immutable typed versions backed by those bytes.
  route("workspace.files.publish", "POST", /^\/workspace\/files$/, "/workspace/files", "files:write"),
  route("workspace.files.list", "GET", /^\/workspace\/files$/, "/workspace/files", "files:read"),
  route("workspace.files.get", "GET", /^\/workspace\/files\/[^/]+$/, "/workspace/files/wres_1", "files:read"),
  route("workspace.files.delete", "DELETE", /^\/workspace\/files\/[^/]+$/, "/workspace/files/wres_1", "files:delete"),
  route("workspace.skills.publish", "POST", /^\/workspace\/skills$/, "/workspace/skills", "skills:write"),
  route("workspace.skills.list", "GET", /^\/workspace\/skills$/, "/workspace/skills", "skills:read"),
  route("workspace.skills.get", "GET", /^\/workspace\/skills\/[^/]+$/, "/workspace/skills/wres_1", "skills:read"),
  route("workspace.skills.delete", "DELETE", /^\/workspace\/skills\/[^/]+$/, "/workspace/skills/wres_1", "skills:delete"),
  route("workspace.tools.publish", "POST", /^\/workspace\/tools$/, "/workspace/tools", "tools:write"),
  route("workspace.tools.list", "GET", /^\/workspace\/tools$/, "/workspace/tools", "tools:read"),
  route("workspace.tools.get", "GET", /^\/workspace\/tools\/[^/]+$/, "/workspace/tools/wres_1", "tools:read"),
  route("workspace.tools.delete", "DELETE", /^\/workspace\/tools\/[^/]+$/, "/workspace/tools/wres_1", "tools:delete"),
  route("workspace.instructions.publish", "POST", /^\/workspace\/instructions$/, "/workspace/instructions", "instructions:write"),
  route("workspace.instructions.list", "GET", /^\/workspace\/instructions$/, "/workspace/instructions", "instructions:read"),
  route("workspace.instructions.get", "GET", /^\/workspace\/instructions\/[^/]+$/, "/workspace/instructions/wres_1", "instructions:read"),
  route("workspace.instructions.delete", "DELETE", /^\/workspace\/instructions\/[^/]+$/, "/workspace/instructions/wres_1", "instructions:delete"),
  // Workspace secret store.
  route("secrets.create", "POST", /^\/secrets$/, "/secrets", "secrets:write"),
  route("secrets.list", "GET", /^\/secrets$/, "/secrets", "secrets:read"),
  route("secrets.rotate", "POST", /^\/secrets\/[^/]+\/rotate$/, "/secrets/NAME/rotate", "secrets:write"),
  route("secrets.delete", "DELETE", /^\/secrets\/[^/]+$/, "/secrets/NAME", "secrets:write"),
  route("secrets.get", "GET", /^\/secrets\/[^/]+$/, "/secrets/NAME", "secrets:read"),
  // Workspace MCP server config.
  route("mcpServers.create", "POST", /^\/mcp-servers$/, "/mcp-servers", "mcp:write"),
  route("mcpServers.list", "GET", /^\/mcp-servers$/, "/mcp-servers", "mcp:read"),
  route("mcpServers.delete", "DELETE", /^\/mcp-servers\/[^/]+$/, "/mcp-servers/mcp_abcdefghi", "mcp:delete"),
  route("mcpServers.get", "GET", /^\/mcp-servers\/[^/]+$/, "/mcp-servers/mcp_abcdefghi", "mcp:read"),
  // Billing (customer read + operator manage).
  route("billing.get", "GET", /^\/billing$/, "/billing", "billing:read"),
  route("billing.ledger", "GET", /^\/billing\/ledger$/, "/billing/ledger", "billing:read"),
  route("billing.portal", "POST", /^\/billing\/portal$/, "/billing/portal", "billing:read"),
  route("adminBilling.topup", "POST", /^\/admin\/billing\/topup$/, "/admin/billing/topup", "billing:manage"),
  route("adminBilling.paymentMethod", "POST", /^\/admin\/billing\/payment-method$/, "/admin/billing/payment-method", "billing:manage"),
  route("adminBilling.accountType", "POST", /^\/admin\/billing\/account-type$/, "/admin/billing/account-type", "billing:manage"),
  // Session webhooks (reveal/rotate signing secret + delivery ledger).
  route("webhook.signingSecret", "POST", /^\/webhook\/signing-secret$/, "/webhook/signing-secret", "sessions:write"),
  route("webhook.listDeliveries", "GET", /^\/webhook\/deliveries$/, "/webhook/deliveries", "sessions:read"),
  // GDPR workspace hard-erase (owner self-service).
  route("workspaces.erase", "DELETE", /^\/workspaces\/[^/]+$/, "/workspaces/ws_1", "workspaces:delete"),
];
