/**
 * The C4 binding table: which data-plane route each response schema describes.
 *
 * The route list is NOT restated here. `api-routes.ts` is the one declaration of
 * the data-plane surface, and this module joins it to the response schemas by
 * operation name — so a route that is added, renamed or removed there shows up
 * here as a coverage change rather than as a silent mismatch. The tests assert
 * that every route name is accounted for in exactly one of
 * {@link DATA_PLANE_RESPONSE_SCHEMAS} or {@link ROUTES_WITHOUT_RESPONSE_SCHEMA}.
 *
 * ## Coverage is a deliverable, not a footnote
 *
 * `04-gates.md`: *"A green C4 over 12 of 120 routes is not a verified surface,
 * and reporting it as one is the specific failure this document exists to
 * prevent."* Three separate facts decide what a green run actually means, and
 * all three are exported rather than implied:
 *
 * 1. **Which routes have a schema** — {@link DATA_PLANE_RESPONSE_SCHEMAS}.
 * 2. **Which do not, and why** — {@link ROUTES_WITHOUT_RESPONSE_SCHEMA}. None is
 *    omitted for convenience; each returns something a JSON response schema
 *    cannot describe, or a shape this package does not own.
 * 3. **Which have a schema that no client in this repo can ever exercise** —
 *    {@link ROUTES_OFF_THE_SDK_SEAM}. These are permanently reported as
 *    "unexercised", and reading that as thin test coverage would be wrong: no
 *    `HttpClient` call site produces them at all.
 *
 * ## One limitation worth knowing before trusting a green run
 *
 * {@link import("./wire-conformance.js").WireResponse} carries a method and a
 * PATH — no origin. The control plane serves different bodies at two of the same
 * paths as the data plane:
 *
 * - `GET /api/whoami` — an `account_token` principal, not an `api_key` one.
 * - `DELETE /api/workspaces/{id}` — the control-plane workspace delete, not the
 *   data-plane GDPR erase.
 *
 * A suite that drives BOTH planes in one process (the CLI does: `aex login` is
 * control-plane) would validate a control-plane body against a data-plane schema
 * and report a violation that is not one. Installing this table in a
 * data-plane-only suite is safe; a mixed suite needs `WireResponse` to carry the
 * origin first.
 */
import type { StandardSchemaV1 } from "@standard-schema/spec";
import {
  AUTHENTICATED_API_ROUTE_DESCRIPTORS,
  type AuthenticatedApiRouteDescriptor
} from "../api-routes.js";
import { NoContentResponseSchema } from "../schemas/response-common.js";
import {
  AssetFinalizeResponseSchema,
  AssetMpuAbortResponseSchema,
  AssetMpuPresignPartsResponseSchema,
  AssetPresignResponseSchema
} from "../schemas/response-assets.js";
import {
  AdminBillingAccountTypeResponseSchema,
  AdminBillingPaymentMethodResponseSchema,
  AdminBillingTopupResponseSchema,
  BillingAutoTopupResponseSchema,
  BillingHostedSessionResponseSchema,
  BillingLedgerResponseSchema,
  BillingSummaryResponseSchema
} from "../schemas/response-billing.js";
import { WhoAmIResponseSchema } from "../schemas/response-identity.js";
import {
  McpServerListResponseSchema,
  McpServerResponseSchema
} from "../schemas/response-mcp-servers.js";
import {
  SecretListResponseSchema,
  SecretResponseSchema
} from "../schemas/response-secrets.js";
import {
  AcknowledgedResponseSchema,
  CoordinatorTicketResponseSchema,
  EventArchiveLinkResponseSchema,
  SessionChildrenResponseSchema,
  SessionDeleteResponseSchema,
  SessionEnvelopeResponseSchema,
  SessionEventsPageResponseSchema,
  SessionFileLinkResponseSchema,
  SessionFilesResponseSchema,
  SessionListResponseSchema,
  SessionMessageAcceptedResponseSchema,
  SessionMessagesPageResponseSchema,
  SessionWebhookDeliveriesResponseSchema
} from "../schemas/response-sessions.js";
import {
  ChildFinalizeResponseSchema,
  ChildResultResponseSchema,
  SessionOtlpResponseSchema
} from "../schemas/response-sessions-internal.js";
import {
  WebhookSigningSecretResponseSchema,
  WorkspaceWebhookDeliveriesResponseSchema
} from "../schemas/response-webhooks.js";
import {
  WorkspaceEraseResponseSchema,
  WorkspaceFilePageResponseSchema,
  WorkspaceFileResponseSchema,
  WorkspaceInstructionPageResponseSchema,
  WorkspaceInstructionResponseSchema,
  WorkspaceSkillPageResponseSchema,
  WorkspaceSkillResponseSchema,
  WorkspaceToolPageResponseSchema,
  WorkspaceToolResponseSchema
} from "../schemas/response-workspace.js";
import type { ResponseSchemaBinding } from "./wire-conformance.js";

/**
 * Turn a dispatch RegExp into a MATCH pattern for
 * {@link import("./wire-conformance.js").pathMatches}.
 *
 * Deliberately not the OpenAPI path form: `pathMatches` only asks whether a
 * segment is a `{…}` placeholder, so naming the parameter would be decoration
 * that has to agree with a second implementation. Cross-reference to the
 * generated document goes through the operation NAME, which both sides carry.
 */
export function routeMatchTemplate(pattern: RegExp): string {
  const source = pattern.source
    .replace(/^\^/, "")
    .replace(/\$$/, "")
    // Collapse variable segments BEFORE unescaping: `[^/]+` contains a literal
    // `/`, so unescaping first tears the class in two.
    .replace(/\[\^\\?\/\]\+/g, "{param}")
    .replace(/\\\//g, "/");
  return source;
}

/**
 * The path the harness sees, which is NOT the path the route table declares.
 *
 * `api-routes.ts` describes the surface as the lambda dispatches it (`/sessions`);
 * every client reaches it under an `/api` prefix, and `HttpClient` reports
 * `url.pathname`. The generated OpenAPI document applies the same prefix.
 */
const API_PREFIX = "/api";

/**
 * Operation name -> the schema its 2xx JSON body must satisfy.
 *
 * Keyed by name so this table and the route table are joined by the identifier
 * both already carry, rather than by a re-stated method and path.
 */
const RESPONSE_SCHEMA_BY_OPERATION: Readonly<Record<string, StandardSchemaV1>> = {
  whoami: WhoAmIResponseSchema,

  // Sessions — every state change answers a `{ session }` envelope.
  "sessions.create": SessionEnvelopeResponseSchema,
  "sessions.get": SessionEnvelopeResponseSchema,
  "sessions.suspend": SessionEnvelopeResponseSchema,
  "sessions.cancel": SessionEnvelopeResponseSchema,
  "sessions.resume": SessionEnvelopeResponseSchema,
  "sessions.requestApproval": SessionEnvelopeResponseSchema,
  "sessions.approve": SessionEnvelopeResponseSchema,
  "sessions.deny": SessionEnvelopeResponseSchema,
  "sessions.list": SessionListResponseSchema,
  "sessions.sendMessage": SessionMessageAcceptedResponseSchema,
  "sessions.delete": SessionDeleteResponseSchema,
  "sessions.listMessages": SessionMessagesPageResponseSchema,
  "sessions.listEvents": SessionEventsPageResponseSchema,
  "sessions.eventsTicket": CoordinatorTicketResponseSchema,
  "sessions.listChildren": SessionChildrenResponseSchema,
  "sessions.eventArchiveLink": EventArchiveLinkResponseSchema,
  "sessions.listFiles": SessionFilesResponseSchema,
  "sessions.fileLink": SessionFileLinkResponseSchema,
  "sessions.listWebhookDeliveries": SessionWebhookDeliveriesResponseSchema,
  "sessions.redeliverWebhook": AcknowledgedResponseSchema,
  "sessions.otel": SessionOtlpResponseSchema,
  "sessions.childResult": ChildResultResponseSchema,
  "sessions.finalize": ChildFinalizeResponseSchema,

  // Assets.
  "assets.presign": AssetPresignResponseSchema,
  "assets.finalize": AssetFinalizeResponseSchema,
  "assets.mpuPresignParts": AssetMpuPresignPartsResponseSchema,
  "assets.mpuAbort": AssetMpuAbortResponseSchema,
  "assets.delete": NoContentResponseSchema,

  // Versioned workspace resources.
  "workspace.files.publish": WorkspaceFileResponseSchema,
  "workspace.files.get": WorkspaceFileResponseSchema,
  "workspace.files.list": WorkspaceFilePageResponseSchema,
  "workspace.files.delete": NoContentResponseSchema,
  "workspace.skills.publish": WorkspaceSkillResponseSchema,
  "workspace.skills.get": WorkspaceSkillResponseSchema,
  "workspace.skills.list": WorkspaceSkillPageResponseSchema,
  "workspace.skills.delete": NoContentResponseSchema,
  "workspace.tools.publish": WorkspaceToolResponseSchema,
  "workspace.tools.get": WorkspaceToolResponseSchema,
  "workspace.tools.list": WorkspaceToolPageResponseSchema,
  "workspace.tools.delete": NoContentResponseSchema,
  "workspace.instructions.publish": WorkspaceInstructionResponseSchema,
  "workspace.instructions.get": WorkspaceInstructionResponseSchema,
  "workspace.instructions.list": WorkspaceInstructionPageResponseSchema,
  "workspace.instructions.delete": NoContentResponseSchema,

  // Workspace secret store.
  "secrets.create": SecretResponseSchema,
  "secrets.get": SecretResponseSchema,
  "secrets.rotate": SecretResponseSchema,
  "secrets.list": SecretListResponseSchema,
  "secrets.delete": NoContentResponseSchema,

  // Workspace MCP server config.
  "mcpServers.create": McpServerResponseSchema,
  "mcpServers.get": McpServerResponseSchema,
  "mcpServers.list": McpServerListResponseSchema,
  "mcpServers.delete": NoContentResponseSchema,

  // Billing.
  "billing.get": BillingSummaryResponseSchema,
  "billing.ledger": BillingLedgerResponseSchema,
  "billing.topupCheckout": BillingHostedSessionResponseSchema,
  "billing.autoTopup": BillingAutoTopupResponseSchema,
  "billing.portal": BillingHostedSessionResponseSchema,
  "adminBilling.topup": AdminBillingTopupResponseSchema,
  "adminBilling.paymentMethod": AdminBillingPaymentMethodResponseSchema,
  "adminBilling.accountType": AdminBillingAccountTypeResponseSchema,

  // Workspace webhooks and GDPR erase.
  "webhook.signingSecret": WebhookSigningSecretResponseSchema,
  "webhook.listDeliveries": WorkspaceWebhookDeliveriesResponseSchema,
  "workspaces.erase": WorkspaceEraseResponseSchema
};

export interface UnschemadRoute {
  readonly name: string;
  readonly reason: string;
}

/**
 * Data-plane routes with NO response schema, and why not.
 *
 * Every entry is here because a JSON response schema cannot describe what the
 * route returns, or because the shape belongs to another package. None is here
 * because it was awkward: an empty schema that asserts nothing would be worse
 * than this list, since it would inflate the "validated" count with routes
 * nothing was checked on.
 */
export const ROUTES_WITHOUT_RESPONSE_SCHEMA: readonly UnschemadRoute[] = [
  {
    name: "sessions.downloadFile",
    reason:
      "Not JSON on success: a small file is 200 raw bytes with the FILE's own content-type, " +
      "a large one is a 302 to presigned storage."
  },
  {
    name: "sessions.archiveInternal",
    reason: "Not JSON on success: 302 with an empty body and content-type application/zip."
  },
  {
    name: "runtime.writerAuthority",
    reason:
      "204 with no body and no headers at all, and writer-token only — it is a liveness probe " +
      "for the writer baton, not a bearer/SDK surface."
  },
  {
    name: "runtime.journalCommit",
    reason:
      "Writer-token only, and its 200 body is the platform's journal control-item shape " +
      "(lambda-runtime-contracts), not a shape this package declares. Three of its six ops " +
      "answer 204 with no body."
  }
];

/**
 * Routes that HAVE a schema but that no client in this repository can produce.
 *
 * They will appear in `unexercised` on every run, forever, and that is not a
 * gap in test coverage — there is no call site to add a test to. Stated so a
 * reader of the report is not misled in the other direction from a false green.
 */
export const ROUTES_OFF_THE_SDK_SEAM: readonly UnschemadRoute[] = [
  {
    name: "sessions.otel",
    reason:
      "`getSessionOtlpPage` reads the body through `HttpClient.download()`, and only " +
      "`HttpClient.request()` reports to the wire observer. Extending the seam to `download()` " +
      "is what would make this observable."
  },
  {
    name: "sessions.childResult",
    reason: "Writer-token only; called by the in-container subagent runtime, not by `HttpClient`."
  },
  {
    name: "sessions.finalize",
    reason: "Writer-token only; the child-settle hop, called from inside the runtime."
  },
  {
    name: "mcpServers.create",
    reason: "No client function exists in `operations.ts` — the dashboard BFF reaches it directly."
  },
  { name: "mcpServers.list", reason: "No client function exists in `operations.ts`." },
  { name: "mcpServers.get", reason: "No client function exists in `operations.ts`." },
  { name: "mcpServers.delete", reason: "No client function exists in `operations.ts`." },
  {
    name: "webhook.listDeliveries",
    reason:
      "No client function exists in `operations.ts`; only the session-scoped " +
      "`sessions.listWebhookDeliveries` has one."
  },
  { name: "adminBilling.topup", reason: "Operator route; no client function in `operations.ts`." },
  {
    name: "adminBilling.paymentMethod",
    reason: "Operator route; no client function in `operations.ts`."
  },
  {
    name: "adminBilling.accountType",
    reason: "Operator route; no client function in `operations.ts`."
  },
  {
    name: "workspaces.erase",
    reason:
      "No DATA-plane client function. `deleteWorkspace()` in `operations.ts` calls the " +
      "identically-shaped CONTROL-plane path — see the plane-collision note above."
  }
];

function bindingFor(
  descriptor: AuthenticatedApiRouteDescriptor
): ResponseSchemaBinding | undefined {
  const schema = RESPONSE_SCHEMA_BY_OPERATION[descriptor.name];
  if (schema === undefined) {
    return undefined;
  }
  return {
    method: descriptor.method.toUpperCase(),
    path: `${API_PREFIX}${routeMatchTemplate(descriptor.pattern)}`,
    name: descriptor.name,
    schema
  };
}

/** Every data-plane route that has a response schema, bound to it. */
export const DATA_PLANE_RESPONSE_SCHEMAS: readonly ResponseSchemaBinding[] =
  AUTHENTICATED_API_ROUTE_DESCRIPTORS.flatMap((descriptor) => {
    const binding = bindingFor(descriptor);
    return binding === undefined ? [] : [binding];
  });

/**
 * Render the STATIC coverage picture — what could be checked, before a single
 * response arrives.
 *
 * Complements `formatWireConformanceReport`, which renders what actually was.
 * Both belong in a suite's output: the dynamic report alone cannot tell a reader
 * whether the 40 operations it validated are most of the surface or a third of
 * it.
 */
export function formatResponseSchemaCoverage(): string {
  const total = AUTHENTICATED_API_ROUTE_DESCRIPTORS.length;
  const bound = DATA_PLANE_RESPONSE_SCHEMAS.length;
  const lines = [
    `response schemas: ${bound}/${total} data-plane route(s) have one`,
    `  NO SCHEMA (${ROUTES_WITHOUT_RESPONSE_SCHEMA.length}):`
  ];
  for (const route of ROUTES_WITHOUT_RESPONSE_SCHEMA) {
    lines.push(`    - ${route.name}: ${route.reason}`);
  }
  lines.push(
    `  SCHEMA BUT UNREACHABLE from any client in this repo — expect these in ` +
      `"NOT EXERCISED" forever (${ROUTES_OFF_THE_SDK_SEAM.length}):`
  );
  for (const route of ROUTES_OFF_THE_SDK_SEAM) {
    lines.push(`    - ${route.name}: ${route.reason}`);
  }
  return lines.join("\n");
}
