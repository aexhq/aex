import { GetSecretValueCommand, SecretsManagerClient } from "@aws-sdk/client-secrets-manager";
import type Stripe from "stripe";

import {
  STRIPE_API_VERSION,
  classifyStripeFailure,
  createStripeClient,
  providerRequestOptions,
} from "./edge.js";
import type { PaymentCommandEnvelope, Succeeded } from "./wire_pending.js";

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
  objectType: string,
  status: string | null | undefined,
  amountCents: number,
  providerRequestId?: string,
): Succeeded {
  return {
    outcome: "succeeded",
    effect: envelope.command.effect,
    objectId,
    objectType,
    status: status ?? "created",
    amountCents,
    currency: "usd",
    ...(providerRequestId === undefined ? {} : { providerRequestId }),
    apiVersion: STRIPE_API_VERSION,
  };
}

/** Executes exactly one generated command with exactly one provider request. */
export async function executePaymentCommand(
  client: Stripe,
  envelope: PaymentCommandEnvelope,
): Promise<Succeeded> {
  const options = providerRequestOptions(envelope.providerIdempotencyKey);
  const effectMetadata = metadata(envelope);
  switch (envelope.command.kind) {
    case "ensure_customer": {
      const object = await client.customers.create(
        { email: envelope.command.email, metadata: effectMetadata },
        options,
      );
      return succeeded(envelope, object.id, "customer", undefined, 0, object.lastResponse.requestId);
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
      return succeeded(envelope, object.id, "checkout_session", object.status, amount, object.lastResponse.requestId);
    }
    case "create_portal_session": {
      const object = await client.billingPortal.sessions.create(
        {
          customer: envelope.command.customer,
          return_url: envelope.command.returnUrl,
        },
        options,
      );
      return succeeded(envelope, object.id, "billing_portal_session", undefined, 0, object.lastResponse.requestId);
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
      return succeeded(envelope, object.id, "payment_intent", object.status, amount, object.lastResponse.requestId);
    }
    case "lookup_effect_outcome": {
      const objects = await client.paymentIntents.search({
        query: `metadata['aex_effect_id']:'${envelope.command.effect}'`,
        limit: 1,
      });
      const object = objects.data[0];
      if (object === undefined) throw new Error("effect outcome was not found");
      return succeeded(
        envelope,
        object.id,
        "payment_intent",
        object.status,
        object.amount,
        objects.lastResponse.requestId,
      );
    }
    case "refund_charge": {
      const amount = cents(envelope.command.amount);
      const object = await client.refunds.create(
        { charge: envelope.command.original, amount, metadata: effectMetadata },
        options,
      );
      return succeeded(envelope, object.id, "refund", object.status, amount, object.lastResponse.requestId);
    }
  }
}

/** Node 22 Lambda entrypoint. */
export const handler = async (event: unknown): Promise<Succeeded | ReturnType<typeof classifyStripeFailure>> => {
  const envelope = envelopeFrom(event);
  const config = configuration();
  const client = createStripeClient(await loadSecret(config.region, config.secretArn));
  try {
    return await executePaymentCommand(client, envelope);
  } catch (error: unknown) {
    return classifyStripeFailure(error);
  }
};
