import { describe, expect, test } from "bun:test";

import {
  STRIPE_API_VERSION,
  STRIPE_CLIENT_OPTIONS,
  classifyStripeFailure,
  providerRequestOptions,
} from "../src/edge.js";

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
      providerRequestId: "req_1",
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
