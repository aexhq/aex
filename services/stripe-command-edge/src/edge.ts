import Stripe from "stripe";

import { STRIPE_API_VERSION } from "./protocol.js";
import type { PaymentCommandFailure } from "./wire_pending.js";

export { STRIPE_API_VERSION };

export const STRIPE_CLIENT_OPTIONS = Object.freeze({
  apiVersion: STRIPE_API_VERSION,
  maxNetworkRetries: 0,
  timeout: 8000,
});

/** Constructs the only admitted provider client. The secret is never logged. */
export function createStripeClient(secret: string): Stripe {
  if (secret.length === 0) throw new Error("Stripe secret is empty");
  // stripe's declarations narrow this field to the package release's latest
  // date, while the runtime deliberately accepts an older explicitly pinned
  // Dahlia date. Keep the value visible and tested above this type-only cast.
  return new Stripe(secret, {
    ...STRIPE_CLIENT_OPTIONS,
    apiVersion: STRIPE_API_VERSION as Stripe.LatestApiVersion,
  });
}

/** Every provider mutation uses the finance-authored idempotency identity. */
export function providerRequestOptions(effectId: string): Readonly<{ idempotencyKey: string }> {
  if (effectId.length === 0) throw new Error("effect id is empty");
  return Object.freeze({ idempotencyKey: effectId });
}

type ProviderFailure = {
  readonly statusCode?: unknown;
  readonly requestId?: unknown;
  readonly type?: unknown;
  readonly code?: unknown;
  readonly decline_code?: unknown;
};

function asRecord(error: unknown): ProviderFailure {
  return typeof error === "object" && error !== null ? error : {};
}

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

/** Total classification preserving outcome uncertainty. */
export function classifyStripeFailure(error: unknown): PaymentCommandFailure {
  const failure = asRecord(error);
  const status = typeof failure.statusCode === "number" ? failure.statusCode : undefined;
  const providerRequestId = optionalString(failure.requestId);
  const code = optionalString(failure.code);
  if (status !== undefined && status >= 500 && status <= 599) {
    return {
      outcome: "indeterminate",
      reason: "provider_5xx",
      ...(providerRequestId === undefined ? {} : { providerRequestId }),
    };
  }
  if (failure.type === "StripeConnectionError") {
    return {
      outcome: "indeterminate",
      reason: code === "ETIMEDOUT" ? "timeout" : "connection_reset",
      ...(providerRequestId === undefined ? {} : { providerRequestId }),
    };
  }
  if (status === 402 || failure.type === "StripeCardError" || status === 400) {
    return {
      outcome: "rejected",
      code: code ?? "invalid_request",
      ...(optionalString(failure.decline_code) === undefined
        ? {}
        : { declineCode: optionalString(failure.decline_code) }),
      errorType: optionalString(failure.type) ?? "StripeInvalidRequestError",
      ...(providerRequestId === undefined ? {} : { providerRequestId }),
      apiVersion: STRIPE_API_VERSION,
    };
  }
  return {
    outcome: "indeterminate",
    reason: status === 429 ? "provider_5xx" : "connection_reset",
    ...(providerRequestId === undefined ? {} : { providerRequestId }),
  };
}
