import { createHash } from "node:crypto";

import { InvokeCommand, LambdaClient } from "@aws-sdk/client-lambda";
import { GetSecretValueCommand, SecretsManagerClient } from "@aws-sdk/client-secrets-manager";
import Stripe from "stripe";

import { STRIPE_API_VERSION } from "../../stripe-command-edge/src/protocol.js";
import {
  decodeRawBody,
  normalizeEvent,
  verifyWithRotatingSecrets,
  type RawRequest,
} from "./edge.js";
import type { ProviderEventEnvelope } from "./wire_pending.js";

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

type Scalar = string | number | boolean;

function record(value: unknown): Readonly<Record<string, unknown>> {
  if (typeof value !== "object" || value === null) throw new Error("event object is invalid");
  return value as Readonly<Record<string, unknown>>;
}

function scalar(value: unknown): Scalar | undefined {
  return typeof value === "string" || typeof value === "number" || typeof value === "boolean"
    ? value
    : undefined;
}

/** The closed, PII-free fact projection. */
function boundedFacts(object: Readonly<Record<string, unknown>>): Readonly<Record<string, Scalar>> {
  const facts: Record<string, Scalar> = {};
  for (const key of [
    "amount",
    "amount_received",
    "currency",
    "status",
    "reason",
    "fee",
    "tax",
    "customer",
    "mode",
    "payment_status",
  ]) {
    const value = scalar(object[key]);
    if (value !== undefined) facts[key] = value;
  }
  const card = object.card;
  if (typeof card === "object" && card !== null) {
    const safeCard = card as Readonly<Record<string, unknown>>;
    for (const key of ["brand", "last4", "exp_month", "exp_year"]) {
      const value = scalar(safeCard[key]);
      if (value !== undefined) facts[`card_${key}`] = value;
    }
  }
  const lastError = object.last_payment_error;
  if (typeof lastError === "object" && lastError !== null) {
    const code = scalar((lastError as Readonly<Record<string, unknown>>).code);
    if (code !== undefined) facts.last_payment_error_code = code;
  }
  return Object.freeze(facts);
}

function normalized(event: Stripe.Event, rawBody: Buffer): ProviderEventEnvelope {
  const object = record(event.data.object);
  const metadataValue = object.metadata;
  const metadata =
    typeof metadataValue === "object" && metadataValue !== null
      ? (metadataValue as Readonly<Record<string, unknown>>)
      : {};
  const organizationId = scalar(metadata.aex_org_id);
  const objectId = scalar(object.id);
  const objectType = scalar(object.object);
  if (typeof objectId !== "string" || typeof objectType !== "string") {
    throw new Error("provider event object identity is missing");
  }
  return {
    providerEventId: event.id,
    eventType: event.type,
    objectId,
    objectType,
    createdAtProvider: event.created,
    apiVersion: STRIPE_API_VERSION,
    rawBodySha256: createHash("sha256").update(rawBody).digest("hex"),
    ...(typeof organizationId === "string" ? { organizationId } : {}),
    facts: boundedFacts(object),
  };
}

/** Node 22 Function URL entrypoint. */
export const handler = async (request: FunctionUrlRequest): Promise<Response> => {
  const region = required("AEX_REGION");
  if (required("AEX_PINNED_STRIPE_API_VERSION") !== STRIPE_API_VERSION) {
    throw new Error("pinned Stripe API version mismatch");
  }
  const secretClient = new SecretsManagerClient({ region });
  const currentArn = required("AEX_WEBHOOK_SECRET_CURRENT_ARN");
  const previousArn = optional("AEX_WEBHOOK_SECRET_PREVIOUS_ARN");
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
        FunctionName: required("AEX_FINANCE_INGEST_ARN"),
        InvocationType: "RequestResponse",
        Payload: Buffer.from(JSON.stringify(normalized(event, rawBody))),
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
