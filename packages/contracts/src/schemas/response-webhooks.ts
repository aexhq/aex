/**
 * Response schemas for the workspace-scoped `webhook.*` routes.
 *
 * The session-scoped delivery ledger lives in `response-sessions.ts`; this
 * module covers the two routes that are workspace-scoped rather than
 * session-scoped. They are not the same shape: the workspace view appends
 * `sessionId` and `callbackUrl` to each row, which the declared
 * `SessionWebhookDelivery` interface does not carry. The workspace view had no
 * declared client type at all; `WorkspaceWebhookDelivery` in `runtime-types.ts`
 * is now `z.infer`red from the schema below.
 */
import * as z from "zod/mini";
import { SessionWebhookDeliverySchema } from "./response-sessions.js";
import {
  describeResponse,
  responseObject,
  wireEnum,
  wireInteger,
  wireNonEmptyString,
  wireNonNegativeInteger,
  wirePositiveInteger,
  wireString
} from "./response-common.js";

const optional = z.optional;

/**
 * `POST /webhook/signing-secret`.
 *
 * One key. The reveal creates the secret on first use and never rotates it, so
 * a second call returns the same value — which is why a strict object matters
 * here: an extra `rotatedAt` or `previous` key would be a semantic change
 * arriving silently.
 */
export const WebhookSigningSecretResponseSchema = describeResponse(
  "WebhookSigningSecretResponse",
  "The workspace webhook signing secret (`whsec_<base64>`), revealed not rotated.",
  responseObject({ whsec: wireNonEmptyString })
);

/** One delivery row in the WORKSPACE view: the session row plus its addressing. */
export const WorkspaceWebhookDeliverySchema = describeResponse(
  "WorkspaceWebhookDelivery",
  "One webhook delivery row with the session and callback it belongs to.",
  responseObject({
    id: wireNonEmptyString,
    runId: wireNonEmptyString,
    turnSeq: wirePositiveInteger,
    eventType: wireEnum(["run.finished", "run.error"]),
    status: wireEnum(["pending", "delivering", "retrying", "delivered", "exhausted", "invalid"]),
    attemptCount: wireNonNegativeInteger,
    createdAt: wireString,
    lastStatusCode: optional(wireInteger),
    lastError: optional(wireString),
    sessionId: wireNonEmptyString,
    callbackUrl: wireNonEmptyString
  })
);

export const WorkspaceWebhookDeliveriesResponseSchema = describeResponse(
  "WorkspaceWebhookDeliveriesResponse",
  "The workspace's 100 most recent webhook deliveries, newest first. " +
    "Server-capped: no `limit`, no cursor.",
  responseObject({ deliveries: z.array(WorkspaceWebhookDeliverySchema) })
);

export type WorkspaceWebhookDelivery = z.infer<typeof WorkspaceWebhookDeliverySchema>;

/** Re-exported so a reader comparing the two views has both names in one place. */
export { SessionWebhookDeliverySchema };
