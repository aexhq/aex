import { describe, expect, test } from "bun:test";

import {
  STRIPE_API_VERSION,
  STRIPE_CLIENT_OPTIONS,
  classifyStripeFailure,
  providerRequestOptions,
} from "../src/payment-edge.js";
import { paymentResultFromFailure } from "../src/payment-handler.js";

describe("pinned Stripe boundary", () => {
  test("pins the API and disables hidden SDK retries", () => {
    expect(STRIPE_API_VERSION).toBe("2026-06-24.dahlia");
    expect(STRIPE_CLIENT_OPTIONS).toEqual({
      apiVersion: STRIPE_API_VERSION,
      maxNetworkRetries: 0,
      timeout: 8000,
    });
    expect(providerRequestOptions("eff_01")).toEqual({ idempotencyKey: "eff_01" });
  });

  test("never classifies a provider 5xx as rejected", () => {
    expect(classifyStripeFailure({ statusCode: 503, requestId: "req_1" })).toEqual({
      outcome: "indeterminate",
      reason: "provider_5xx",
      status: 503,
      providerRequestId: "req_1",
    });
  });

  test("maps provider observations onto the exact Rust PaymentResult shape", () => {
    expect(
      paymentResultFromFailure("eff_1", {
        outcome: "rejected",
        code: "card_declined",
        declineCode: "insufficient_funds",
        errorType: "StripeCardError",
        apiVersion: STRIPE_API_VERSION,
      }),
    ).toEqual({
      outcome: "failed",
      effect: "eff_1",
      failure: {
        class: "card_declined",
        providerCode: "card_declined",
        declineCode: "insufficient_funds",
        retryable: false,
      },
    });
    expect(
      paymentResultFromFailure("eff_1", {
        outcome: "indeterminate",
        reason: "provider_5xx",
        status: 502,
      }),
    ).toEqual({
      outcome: "unknown",
      effect: "eff_1",
      evidence: { evidence: "server_error", status: 502 },
    });
  });

  test("classifies a card decline as a determinate rejection", () => {
    expect(
      classifyStripeFailure({
        statusCode: 402,
        type: "StripeCardError",
        code: "card_declined",
        decline_code: "insufficient_funds",
        requestId: "req_2",
      }),
    ).toEqual({
      outcome: "rejected",
      code: "card_declined",
      declineCode: "insufficient_funds",
      errorType: "StripeCardError",
      providerRequestId: "req_2",
      apiVersion: STRIPE_API_VERSION,
    });
  });

  test("treats timeout and connection loss as indeterminate", () => {
    expect(classifyStripeFailure({ type: "StripeConnectionError", code: "ETIMEDOUT" })).toEqual({
      outcome: "indeterminate",
      reason: "timeout",
    });
    expect(classifyStripeFailure({ type: "StripeConnectionError", code: "ECONNRESET" })).toEqual({
      outcome: "indeterminate",
      reason: "connection_reset",
    });
  });
});
