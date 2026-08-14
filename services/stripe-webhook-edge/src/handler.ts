import { createHash } from "node:crypto";

import { InvokeCommand, LambdaClient } from "@aws-sdk/client-lambda";
import { GetSecretValueCommand, SecretsManagerClient } from "@aws-sdk/client-secrets-manager";
import Stripe from "stripe";

import { paymentHandler } from "./payment-handler.js";
import {
  STRIPE_API_VERSION,
  type PaymentCommandEnvelope,
  type PaymentResult,
} from "./protocol.js";
import {
  decodeRawBody,
  normalizeEvent,
  verifyWithRotatingSecrets,
  type RawRequest,
} from "./edge.js";
import type {
  CardBrand,
  IngestRequest,
  ProviderEventFacts,
  ProviderEventKind,
} from "./wire.js";

interface FunctionUrlRequest extends RawRequest {
  readonly headers?: Readonly<Record<string, string | undefined>>;
}

interface Response {
  readonly statusCode: number;
  readonly body: string;
}

function required(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.trim().length === 0) throw new Error(`${name} is required`);
  return value;
}

function optional(name: string): string | undefined {
  const value = process.env[name];
  return value === undefined || value.trim().length === 0 ? undefined : value;
}

function signature(headers: FunctionUrlRequest["headers"]): string {
  for (const [name, value] of Object.entries(headers ?? {})) {
    if (name.toLowerCase() === "stripe-signature" && value !== undefined && value.length > 0) {
      return value;
    }
  }
  throw new Error("Stripe-Signature header is required");
}

async function secret(client: SecretsManagerClient, arn: string): Promise<string> {
  const response = await client.send(new GetSecretValueCommand({ SecretId: arn }));
  if (response.SecretString === undefined || response.SecretString.length === 0) {
    throw new Error("webhook secret has no SecretString");
  }
  return response.SecretString;
}

function record(value: unknown): Readonly<Record<string, unknown>> {
  if (typeof value !== "object" || value === null) throw new Error("event object is invalid");
  return value as Readonly<Record<string, unknown>>;
}

function requiredString(value: unknown, field: string): string {
  if (typeof value !== "string" || value.length === 0) throw new Error(`${field} is missing`);
  return value;
}

function optionalRecord(value: unknown): Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null ? record(value) : {};
}

function integer(value: unknown, field: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new Error(`${field} is not a non-negative safe integer`);
  }
  return value;
}

function metadataInteger(value: unknown, field: string): number {
  if (typeof value === "string" && /^\d+$/.test(value)) {
    const parsed = Number(value);
    if (Number.isSafeInteger(parsed)) return parsed;
  }
  return integer(value, field);
}

function instant(seconds: unknown, field: string): string {
  const value = integer(seconds, field);
  const rendered = new Date(value * 1000).toISOString();
  if (rendered === "Invalid Date") throw new Error(`${field} is outside the timestamp bound`);
  return rendered;
}

function reference(value: unknown, field: string): string {
  if (typeof value === "string") return requiredString(value, field);
  return requiredString(record(value).id, `${field}.id`);
}

function metadata(object: Readonly<Record<string, unknown>>): Readonly<Record<string, unknown>> {
  const value = object.metadata;
  return typeof value === "object" && value !== null ? record(value) : {};
}

function organization(object: Readonly<Record<string, unknown>>): string {
  return requiredString(metadata(object).aex_org_id, "metadata.aex_org_id");
}

function effect(object: Readonly<Record<string, unknown>>): string | undefined {
  const value = metadata(object).aex_effect_id;
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function cardBrand(value: unknown): CardBrand {
  switch (value) {
    case "visa":
    case "mastercard":
    case "amex":
    case "discover":
    case "diners":
    case "jcb":
    case "unionpay":
      return value;
    default:
      return "unknown";
  }
}

function methodDisplay(object: Readonly<Record<string, unknown>>) {
  if (object.type !== "card") throw new Error("only card payment methods are supported");
  const card = record(object.card);
  const last4 = requiredString(card.last4, "card.last4");
  if (!/^\d{4}$/.test(last4)) throw new Error("card.last4 is not four decimal digits");
  const expiryMonth = integer(card.exp_month, "card.exp_month");
  const expiryYear = integer(card.exp_year, "card.exp_year");
  if (expiryMonth < 1 || expiryMonth > 12 || expiryYear < 2020 || expiryYear > 9999) {
    throw new Error("card expiry is outside its display bound");
  }
  return {
    customer: reference(object.customer, "customer"),
    method: requiredString(object.id, "payment_method.id"),
    brand: cardBrand(card.brand),
    last4,
    expiryMonth,
    expiryYear,
    providerCreatedAt: instant(object.created, "payment_method.created"),
  } as const;
}

function facts(eventType: string, object: Readonly<Record<string, unknown>>): ProviderEventFacts {
  switch (eventType) {
    case "payment_method.attached":
      return { kind: "payment_method_attached", ...methodDisplay(object) };
    case "payment_method.updated":
      return { kind: "payment_method_updated", ...methodDisplay(object) };
    case "payment_method.detached":
      return {
        kind: "payment_method_detached",
        method: requiredString(object.id, "payment_method.id"),
      };
    case "payment_intent.succeeded":
      return {
        kind: "payment_intent_succeeded",
        organization: organization(object),
        credit: String(
          metadataInteger(metadata(object).aex_credit_cents, "metadata.aex_credit_cents"),
        ),
        charged: String(integer(object.amount_received, "payment_intent.amount_received")),
        intent: requiredString(object.id, "payment_intent.id"),
      };
    case "payment_intent.payment_failed": {
      const error = optionalRecord(object.last_payment_error);
      const code = typeof error.code === "string" ? error.code : null;
      const failureClass =
        code === "card_declined"
          ? "card_declined"
          : code === "authentication_required"
            ? "authentication_required"
            : "invalid_request";
      return {
        kind: "payment_intent_payment_failed",
        organization: organization(object),
        intent: requiredString(object.id, "payment_intent.id"),
        failure: {
          class: failureClass,
          providerCode: code,
          declineCode: typeof error.decline_code === "string" ? error.decline_code : null,
          retryable: false,
        },
      };
    }
    case "charge.dispute.created":
      return {
        kind: "charge_dispute_created",
        organization: organization(object),
        charge: reference(object.charge, "dispute.charge"),
        amount: String(integer(object.amount, "dispute.amount")),
      };
    case "refund.created":
    case "refund.failed":
      return {
        kind: eventType === "refund.created" ? "refund_created" : "refund_failed",
        organization: organization(object),
        charge: reference(object.charge, "refund.charge"),
        refund: requiredString(object.id, "refund.id"),
        amount: String(integer(object.amount, "refund.amount")),
      };
    default:
      throw new Error(`event type ${eventType} is not in the finance-ingest contract`);
  }
}

const KINDS: Readonly<Record<string, ProviderEventKind>> = {
  "payment_method.attached": "payment_method_attached",
  "payment_method.updated": "payment_method_updated",
  "payment_method.detached": "payment_method_detached",
  "payment_intent.succeeded": "payment_intent_succeeded",
  "payment_intent.payment_failed": "payment_intent_payment_failed",
  "charge.dispute.created": "charge_dispute_created",
  "refund.created": "refund_created",
  "refund.failed": "refund_failed",
};

/** Reduces a verified Stripe event to the exact Rust finance-ingest request. */
export function ingestRequest(
  event: Stripe.Event,
  rawBody: Buffer,
  receivedAt: Date = new Date(),
): IngestRequest {
  const object = record(event.data.object);
  const kind = KINDS[event.type];
  if (kind === undefined) throw new Error(`event type ${event.type} is not handled`);
  const projected = facts(event.type, object);
  if (projected.kind !== kind) throw new Error("event kind projection mismatch");
  const projectedEffect = effect(object);
  return {
    request: "provider_event",
    event: {
      schemaVersion: 1,
      providerEventId: event.id,
      object: requiredString(object.id, "event object.id"),
      kind,
      occurredAt: instant(event.created, "event.created"),
      facts: projected,
      rawDigest: `sha256:${createHash("sha256").update(rawBody).digest("hex")}`,
      providerApiVersion: STRIPE_API_VERSION,
      ...(projectedEffect === undefined ? {} : { effect: projectedEffect }),
      receivedAt: receivedAt.toISOString(),
    },
  };
}

/** Node 22 Function URL entrypoint. */
const webhookHandler = async (request: FunctionUrlRequest): Promise<Response> => {
  const region = required("AEX_REGION");
  if (required("AEX_STRIPE_API_VERSION") !== STRIPE_API_VERSION) {
    throw new Error("pinned Stripe API version mismatch");
  }
  const secretClient = new SecretsManagerClient({ region });
  const currentArn = required("AEX_STRIPE_WEBHOOK_SECRET_CURRENT_ARN");
  const previousArn = optional("AEX_STRIPE_WEBHOOK_SECRET_PREVIOUS_ARN");
  const secrets = [
    await secret(secretClient, currentArn),
    ...(previousArn === undefined ? [] : [await secret(secretClient, previousArn)]),
  ];
  let rawBody: Buffer;
  let event: Stripe.Event;
  try {
    rawBody = decodeRawBody(request);
    const stripe = new Stripe("sk_not_used_for_webhook_verification", {
      apiVersion: STRIPE_API_VERSION as Stripe.LatestApiVersion,
      maxNetworkRetries: 0,
    });
    event = verifyWithRotatingSecrets(rawBody, signature(request.headers), secrets, (body, sig, key) =>
      stripe.webhooks.constructEvent(body, sig, key),
    );
  } catch (error: unknown) {
    const message = error instanceof Error ? error.message : "invalid webhook";
    return { statusCode: 400, body: JSON.stringify({ error: message }) };
  }
  const disposition = normalizeEvent(event);
  if (disposition.disposition !== "forward") {
    return { statusCode: 200, body: JSON.stringify(disposition) };
  }
  const lambda = new LambdaClient({ region });
  try {
    const result = await lambda.send(
      new InvokeCommand({
        FunctionName: required("AEX_BILLING_WORKER_FUNCTION_ARN"),
        InvocationType: "RequestResponse",
        Payload: Buffer.from(JSON.stringify(ingestRequest(event, rawBody))),
      }),
    );
    if (result.FunctionError !== undefined || result.StatusCode !== 200) {
      throw new Error("finance ingest did not durably accept the event");
    }
  } catch {
    return { statusCode: 500, body: JSON.stringify({ error: "finance_ingest_unavailable" }) };
  }
  return { statusCode: 200, body: JSON.stringify({ appliedState: "accepted" }) };
};

function isPaymentCommand(value: unknown): value is PaymentCommandEnvelope {
  if (typeof value !== "object" || value === null) return false;
  const candidate = value as Partial<PaymentCommandEnvelope>;
  return (
    typeof candidate.providerIdempotencyKey === "string" &&
    typeof candidate.command === "object" &&
    candidate.command !== null
  );
}

/** Exact Stripe boundary: public Function URL webhooks and IAM-only payment commands. */
export function handler(request: FunctionUrlRequest): Promise<Response>;
export function handler(request: PaymentCommandEnvelope): Promise<PaymentResult>;
export function handler(
  request: FunctionUrlRequest | PaymentCommandEnvelope,
): Promise<Response | PaymentResult> {
  return isPaymentCommand(request) ? paymentHandler(request) : webhookHandler(request);
}
