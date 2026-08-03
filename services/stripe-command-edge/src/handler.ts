import { GetSecretValueCommand, SecretsManagerClient } from "@aws-sdk/client-secrets-manager";
import type Stripe from "stripe";

import {
  STRIPE_API_VERSION,
  classifyStripeFailure,
  createStripeClient,
  providerRequestOptions,
} from "./edge.js";
import type {
  Failed,
  HostedSession,
  PaymentCommandEnvelope,
  PaymentCommandFailure,
  PaymentResult,
  Succeeded,
  Unknown,
} from "./wire_pending.js";

const COMMAND_KINDS = new Set([
  "ensure_customer",
  "create_top_up_checkout",
  "create_portal_session",
  "charge_saved_method",
  "lookup_effect_outcome",
  "refund_charge",
]);

function required(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.trim().length === 0) throw new Error(`${name} is required`);
  return value;
}

function configuration(): { readonly region: string; readonly secretArn: string } {
  const apiVersion = required("AEX_PINNED_STRIPE_API_VERSION");
  if (apiVersion !== STRIPE_API_VERSION) throw new Error("pinned Stripe API version mismatch");
  if (required("AEX_REQUEST_TIMEOUT_MS") !== "8000") {
    throw new Error("AEX_REQUEST_TIMEOUT_MS must be exactly 8000");
  }
  return { region: required("AEX_REGION"), secretArn: required("AEX_STRIPE_SECRET_ARN") };
}

async function loadSecret(region: string, secretArn: string): Promise<string> {
  const result = await new SecretsManagerClient({ region }).send(
    new GetSecretValueCommand({ SecretId: secretArn }),
  );
  if (result.SecretString === undefined || result.SecretString.length === 0) {
    throw new Error("Stripe secret has no SecretString");
  }
  return result.SecretString;
}

function envelopeFrom(value: unknown): PaymentCommandEnvelope {
  if (typeof value !== "object" || value === null) throw new Error("command must be an object");
  const envelope = value as Partial<PaymentCommandEnvelope>;
  if (
    typeof envelope.providerIdempotencyKey !== "string" ||
    envelope.providerIdempotencyKey.length === 0 ||
    typeof envelope.deadline !== "string" ||
    typeof envelope.command !== "object" ||
    envelope.command === null ||
    !COMMAND_KINDS.has(envelope.command.kind) ||
    typeof envelope.metadata !== "object" ||
    envelope.metadata === null
  ) {
    throw new Error("command envelope is invalid");
  }
  const deadline = Date.parse(envelope.deadline);
  if (!Number.isFinite(deadline) || deadline <= Date.now()) throw new Error("command deadline elapsed");
  if (
    envelope.metadata.effect !== envelope.command.effect ||
    envelope.metadata.kind !== envelope.command.kind
  ) {
    throw new Error("effect metadata does not match command");
  }
  if (
    envelope.command.kind === "lookup_effect_outcome" &&
    (envelope.command.expect !== "charge_saved_method" ||
      (envelope.command.provider !== null && typeof envelope.command.provider !== "string"))
  ) {
    throw new Error("lookup command is unsupported");
  }
  return envelope as PaymentCommandEnvelope;
}

function metadata(envelope: PaymentCommandEnvelope): Stripe.MetadataParam {
  return {
    aex_effect_id: envelope.metadata.effect,
    aex_org_id: envelope.metadata.organization,
    aex_command_kind: envelope.metadata.kind,
    aex_credit_cents: String(envelope.metadata.credit),
  };
}

function cents(value: number): number {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error("amount must be integer cents");
  return value;
}

function succeeded(
  envelope: PaymentCommandEnvelope,
  objectId: string,
  createdSeconds: number,
  amountCents: number,
  hosted: HostedSession | null = null,
): Succeeded {
  return {
    outcome: "succeeded",
    effect: envelope.command.effect,
    providerRef: objectId,
    providerCreatedAt: new Date(createdSeconds * 1000).toISOString(),
    hosted,
    charged: amountCents,
    tax: null,
  };
}

function failed(effect: string, failure: Extract<PaymentCommandFailure, { outcome: "rejected" }>): Failed {
  const classification =
    failure.code === "authentication_required" || failure.declineCode === "authentication_required"
      ? "authentication_required"
      : failure.errorType === "StripeCardError"
        ? "card_declined"
        : "invalid_request";
  return {
    outcome: "failed",
    effect,
    failure: {
      class: classification,
      providerCode: failure.code,
      declineCode: failure.declineCode ?? null,
      retryable: false,
    },
  };
}

function unknown(
  effect: string,
  failure: Extract<PaymentCommandFailure, { outcome: "indeterminate" }>,
): Unknown {
  switch (failure.reason) {
    case "timeout":
      return { outcome: "unknown", effect, evidence: { evidence: "timeout", waitedMs: 8000 } };
    case "provider_5xx":
      return {
        outcome: "unknown",
        effect,
        evidence: { evidence: "server_error", status: failure.status ?? 500 },
      };
    case "rate_limited":
      return {
        outcome: "unknown",
        effect,
        evidence: { evidence: "ambiguous_response", providerCode: "rate_limited" },
      };
    case "connection_reset":
      return { outcome: "unknown", effect, evidence: { evidence: "transport_lost" } };
  }
}

export function paymentResultFromFailure(
  effect: string,
  failure: PaymentCommandFailure,
): Failed | Unknown {
  return failure.outcome === "rejected" ? failed(effect, failure) : unknown(effect, failure);
}

/** Executes exactly one generated command with exactly one provider request. */
export async function executePaymentCommand(
  client: Stripe,
  envelope: PaymentCommandEnvelope,
): Promise<PaymentResult> {
  const options = providerRequestOptions(envelope.providerIdempotencyKey);
  const effectMetadata = metadata(envelope);
  switch (envelope.command.kind) {
    case "ensure_customer": {
      const object = await client.customers.create(
        { email: envelope.command.email, metadata: effectMetadata },
        options,
      );
      return succeeded(envelope, object.id, object.created, 0);
    }
    case "create_top_up_checkout": {
      const amount = cents(envelope.command.amount);
      const object = await client.checkout.sessions.create(
        {
          mode: "payment",
          customer: envelope.command.customer,
          customer_update: { address: "auto" },
          billing_address_collection: "required",
          automatic_tax: { enabled: envelope.command.tax === "provider_automatic" },
          payment_intent_data: { setup_future_usage: "off_session", metadata: effectMetadata },
          client_reference_id: envelope.command.organization,
          success_url: envelope.command.successUrl,
          cancel_url: envelope.command.cancelUrl,
          line_items: [
            {
              quantity: 1,
              price_data: {
                currency: "usd",
                tax_behavior: "exclusive",
                unit_amount: amount,
                product_data: { name: "AEX prepaid credit", metadata: effectMetadata },
              },
            },
          ],
          metadata: effectMetadata,
        },
        options,
      );
      return succeeded(
        envelope,
        object.id,
        object.created,
        amount,
        object.url === null
          ? null
          : { url: object.url, expiresAt: new Date(object.expires_at * 1000).toISOString() },
      );
    }
    case "create_portal_session": {
      const object = await client.billingPortal.sessions.create(
        {
          customer: envelope.command.customer,
          return_url: envelope.command.returnUrl,
        },
        options,
      );
      return succeeded(envelope, object.id, object.created, 0, {
        url: object.url,
        // Stripe does not publish an expiry timestamp for portal sessions. AEX
        // exposes a conservative five-minute grant rather than claiming the
        // provider URL is durable.
        expiresAt: new Date((object.created + 300) * 1000).toISOString(),
      });
    }
    case "charge_saved_method": {
      const amount = cents(envelope.command.amount);
      const object = await client.paymentIntents.create(
        {
          amount,
          currency: "usd",
          customer: envelope.command.customer,
          payment_method: envelope.command.method,
          confirm: true,
          off_session: true,
          metadata: effectMetadata,
        },
        options,
      );
      if (object.status === "succeeded") {
        return succeeded(envelope, object.id, object.created, amount);
      }
      if (object.status === "requires_action") {
        return {
          outcome: "failed",
          effect: envelope.command.effect,
          failure: {
            class: "authentication_required",
            providerCode: object.status,
            declineCode: null,
            retryable: false,
          },
        };
      }
      return {
        outcome: "unknown",
        effect: envelope.command.effect,
        evidence: { evidence: "ambiguous_response", providerCode: object.status },
      };
    }
    case "lookup_effect_outcome": {
      if (envelope.command.expect !== "charge_saved_method") {
        return {
          outcome: "unknown",
          effect: envelope.command.effect,
          evidence: { evidence: "ambiguous_response", providerCode: "unsupported_lookup_kind" },
        };
      }
      const object =
        envelope.command.provider === null
          ? (
              await client.paymentIntents.search({
                query: `metadata['aex_effect_id']:'${envelope.command.effect}'`,
                limit: 1,
              })
            ).data[0]
          : await client.paymentIntents.retrieve(envelope.command.provider);
      if (object?.metadata.aex_effect_id !== envelope.command.effect) {
        return {
          outcome: "unknown",
          effect: envelope.command.effect,
          evidence: { evidence: "ambiguous_response", providerCode: "effect_metadata_mismatch" },
        };
      }
      if (object === undefined || object.status !== "succeeded") {
        return {
          outcome: "unknown",
          effect: envelope.command.effect,
          evidence: { evidence: "ambiguous_response", providerCode: object?.status ?? null },
        };
      }
      return succeeded(envelope, object.id, object.created, object.amount);
    }
    case "refund_charge": {
      const amount = cents(envelope.command.amount);
      const object = await client.refunds.create(
        { charge: envelope.command.original, amount, metadata: effectMetadata },
        options,
      );
      if (object.status !== "succeeded") {
        return {
          outcome: "unknown",
          effect: envelope.command.effect,
          evidence: { evidence: "ambiguous_response", providerCode: object.status ?? null },
        };
      }
      return succeeded(envelope, object.id, object.created, amount);
    }
  }
}

/** Node 22 Lambda entrypoint. */
export const handler = async (event: unknown): Promise<PaymentResult> => {
  const envelope = envelopeFrom(event);
  const config = configuration();
  const client = createStripeClient(await loadSecret(config.region, config.secretArn));
  try {
    return await executePaymentCommand(client, envelope);
  } catch (error: unknown) {
    return paymentResultFromFailure(envelope.command.effect, classifyStripeFailure(error));
  }
};
