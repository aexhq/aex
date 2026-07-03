import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";

interface RecordedCall {
  readonly url: string;
  readonly method: string;
  readonly body?: string;
}

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" }
  });
}

function billingClient(body: unknown): { readonly client: Aex; readonly calls: RecordedCall[] } {
  const calls: RecordedCall[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const requestBody = init?.body?.toString();
    calls.push({
      url,
      method: (init?.method ?? "GET").toString(),
      ...(requestBody !== undefined ? { body: requestBody } : {})
    });
    return json(body);
  };
  return {
    client: new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch }),
    calls
  };
}

describe("aex.billing", () => {
  it("GETs /api/billing and returns the typed summary", async () => {
    const summary = {
      balanceUsd: 12.5,
      monthSpendUsd: 0.42,
      spendCapUsd: 50,
      planKey: "free",
      subscriptionStatus: "none"
    };
    const { client, calls } = billingClient(summary);

    const result = await client.billing();

    expect(result).toEqual(summary);
    expect(calls).toHaveLength(1);
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/billing");
    expect(url.search).toBe("");
    expect(calls[0]!.method).toBe("GET");
  });

  it("tolerates additive server fields on the summary (no strict-reject)", async () => {
    const summary = {
      balanceUsd: 1,
      monthSpendUsd: 0,
      spendCapUsd: 10,
      planKey: "free",
      subscriptionStatus: "none",
      paymentMethodStatus: "active",
      accountType: "team",
      plan: { name: "future-field" }
    };
    const { client } = billingClient(summary);

    const result = await client.billing();

    // Unknown additive keys pass through untouched.
    expect(result).toEqual(summary);
    expect(result.paymentMethodStatus).toBe("active");
    expect(result["accountType"]).toBe("team");
  });
});

describe("aex.billingCheckout", () => {
  it("POSTs /api/billing/checkout and returns the hosted URL", async () => {
    const { client, calls } = billingClient({ url: "https://checkout.stripe.test/session" });

    const result = await client.billingCheckout({
      planKey: "pro",
      successUrl: "https://aex.dev/billing?checkout=success",
      cancelUrl: "https://aex.dev/billing?checkout=cancel",
      idempotencyKey: "checkout-key"
    });

    expect(result).toEqual({ url: "https://checkout.stripe.test/session" });
    expect(calls).toHaveLength(1);
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/billing/checkout");
    expect(calls[0]!.method).toBe("POST");
    expect(JSON.parse(calls[0]!.body ?? "{}")).toEqual({
      planKey: "pro",
      successUrl: "https://aex.dev/billing?checkout=success",
      cancelUrl: "https://aex.dev/billing?checkout=cancel",
      idempotencyKey: "checkout-key"
    });
  });
});

describe("aex.billingPortal", () => {
  it("POSTs /api/billing/portal and returns the hosted URL", async () => {
    const { client, calls } = billingClient({ url: "https://billing.stripe.test/session" });

    const result = await client.billingPortal({ returnUrl: "https://aex.dev/billing" });

    expect(result).toEqual({ url: "https://billing.stripe.test/session" });
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/billing/portal");
    expect(calls[0]!.method).toBe("POST");
    expect(JSON.parse(calls[0]!.body ?? "{}")).toEqual({ returnUrl: "https://aex.dev/billing" });
  });
});

describe("aex.billingLedger", () => {
  it("GETs /api/billing/ledger and threads the limit param", async () => {
    const page = {
      entries: [
        {
          id: "led-1",
          entryType: "top_up",
          amountUsd: 10,
          currency: "USD",
          runId: null,
          description: "admin top-up",
          createdBy: "admin:ops@example.test",
          createdAt: "2026-07-01T00:00:00Z"
        }
      ]
    };
    const { client, calls } = billingClient(page);

    const result = await client.billingLedger({ limit: 5 });

    expect(result).toEqual(page);
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/billing/ledger");
    expect(url.searchParams.get("limit")).toBe("5");
    expect(calls[0]!.method).toBe("GET");
  });

  it("omits the limit param when no query is given", async () => {
    const { client, calls } = billingClient({ entries: [] });
    await client.billingLedger();
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/billing/ledger");
    expect(url.search).toBe("");
  });
});

describe("aex.webhookSigningSecret", () => {
  it("POSTs /api/webhook/signing-secret and returns the whsec reveal", async () => {
    const { client, calls } = billingClient({ whsec: "whsec_dGVzdC1zZWNyZXQ=" });

    const result = await client.webhookSigningSecret();

    expect(result).toEqual({ whsec: "whsec_dGVzdC1zZWNyZXQ=" });
    expect(calls).toHaveLength(1);
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/webhook/signing-secret");
    expect(calls[0]!.method).toBe("POST");
  });
});
