import { describe, expect, it } from "bun:test";
import { idPattern, isId, type FetchLike } from "@aexhq/contracts";
import { Aex } from "../../src/index.js";

interface RecordedCall {
  readonly url: string;
  readonly method: string;
  readonly headers: Headers;
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
  const fetch: FetchLike = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const requestBody = init?.body?.toString();
    calls.push({
      url,
      method: (init?.method ?? "GET").toString(),
      headers: new Headers(init?.headers),
      ...(requestBody !== undefined ? { body: requestBody } : {})
    });
    return json(body);
  };
  return {
    client: new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch }),
    calls
  };
}

const SUMMARY = {
  balanceUsd: 12.5,
  monthSpendUsd: 0.42,
  spendCapUsd: 50,
  period: "2026-07",
  admissionState: "carded_manual" as const,
  accountType: "standard" as const,
  paymentMethodStatus: "active" as const,
  autoTopupEnabled: false,
  blocked: null,
  paymentMethod: { present: true, brand: "visa", last4: "4242" },
  autoTopup: { enabled: false, thresholdUsd: 5, amountUsd: 20, minimumAmountUsd: 10, maxPerDay: 4 },
  allowances: [
    {
      dimension: "llm_token_usd",
      quota: 2,
      used: 0.5,
      remaining: 1.5,
      unit: "USD",
      label: "model usage",
      resetAt: "2026-08-01T00:00:00.000Z"
    }
  ]
};

describe("aex.billing", () => {
  it("GETs /api/billing and returns the typed prepaid summary", async () => {
    const { client, calls } = billingClient(SUMMARY);

    const result = await client.billing();

    expect(result).toEqual(SUMMARY);
    // The prepaid surface, typed without casts.
    expect(result.admissionState).toBe("carded_manual");
    expect(result.autoTopup.minimumAmountUsd).toBe(10);
    expect(result.paymentMethod.last4).toBe("4242");
    expect(result.allowances[0]!.remaining).toBe(1.5);
    expect(result.allowances[0]!.unit).toBe("USD");
    expect(calls).toHaveLength(1);
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/billing");
    expect(url.search).toBe("");
    expect(calls[0]!.method).toBe("GET");
  });

  it("passes an undeclared server field through the read without rejecting it", async () => {
    // `getBilling` does not parse; it hands the decoded body back, so an
    // undeclared key still arrives at RUN TIME. What changed is that
    // `BillingSummary` no longer DECLARES that it will: `[key: string]: unknown`
    // made every undeclared field structurally legal, and that is how
    // `accountType` went missing from the interface while the server sent it on
    // every call. Reaching one now takes an explicit widening.
    const summary = { ...SUMMARY, statements: { available: true } };
    const { client } = billingClient(summary);

    const result = await client.billing();

    expect(result).toEqual(summary);
    expect(result.paymentMethodStatus).toBe("active");
    expect(result.accountType).toBe("standard");
    // The double cast IS the proof: with the index signature gone, reaching an
    // undeclared key is no longer something the type quietly allows.
    expect((result as unknown as Record<string, unknown>)["statements"]).toEqual({
      available: true
    });
  });
});

describe("aex.billingTopup", () => {
  it("POSTs /api/billing/topup/checkout and returns the hosted URL", async () => {
    const { client, calls } = billingClient({ url: "https://checkout.stripe.test/session" });

    const result = await client.billingTopup({
      amountUsd: 25,
      successUrl: "https://aex.dev/billing?checkout=success",
      cancelUrl: "https://aex.dev/billing?checkout=cancel"
    });

    expect(result).toEqual({ url: "https://checkout.stripe.test/session" });
    expect(calls).toHaveLength(1);
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/billing/topup/checkout");
    expect(calls[0]!.method).toBe("POST");
    expect(JSON.parse(calls[0]!.body ?? "{}")).toEqual({
      amountUsd: 25,
      successUrl: "https://aex.dev/billing?checkout=success",
      cancelUrl: "https://aex.dev/billing?checkout=cancel"
    });
    // SDK-minted: the caller never supplied one, but the wire still carries it.
    expect(calls[0]!.headers.get("idempotency-key")).toMatch(idPattern("idempotency"));
  });

  it("reuses one generated identity across transport retries", async () => {
    const identities: string[] = [];
    const bodies: unknown[] = [];
    let attempt = 0;
    const client = new Aex({
      apiKey: "tkn",
      baseUrl: "https://example.test",
      retry: { maxAttempts: 2, initialDelayMs: 1, maxDelayMs: 1, maxElapsedMs: 1_000 },
      fetch: async (_input, init) => {
        identities.push(new Headers(init?.headers).get("idempotency-key") ?? "");
        bodies.push(JSON.parse(String(init?.body ?? "{}")));
        attempt += 1;
        if (attempt === 1) {
          return new Response(JSON.stringify({ error: "temporarily_unavailable" }), {
            status: 503,
            headers: { "content-type": "application/json", "retry-after": "0" }
          });
        }
        return json({ url: "https://billing.stripe.test/session" });
      }
    });

    // A retried top-up must not become two charges: the identity is generated
    // ONCE, before the first attempt, and reused verbatim.
    await client.billingTopup({ amountUsd: 25 });

    expect(identities).toHaveLength(2);
    expect(isId("idempotency", identities[0])).toBe(true);
    expect(identities[1]).toBe(identities[0]);
    expect(bodies).toEqual([{ amountUsd: 25 }, { amountUsd: 25 }]);
  });
});

describe("aex.billingAutoTopup", () => {
  it("PATCHes /api/billing/autotopup with only the fields given", async () => {
    const autoTopup = { enabled: true, thresholdUsd: 5, amountUsd: 20, minimumAmountUsd: 10, maxPerDay: 4 };
    const { client, calls } = billingClient({ autoTopup });

    const result = await client.billingAutoTopup({ enabled: true, thresholdUsd: 5, amountUsd: 20 });

    expect(result.autoTopup).toEqual(autoTopup);
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/billing/autotopup");
    expect(calls[0]!.method).toBe("PATCH");
    expect(JSON.parse(calls[0]!.body ?? "{}")).toEqual({ enabled: true, thresholdUsd: 5, amountUsd: 20 });
  });

  it("sends a partial update verbatim — an omitted field keeps its stored value", async () => {
    const { client, calls } = billingClient({
      autoTopup: { enabled: false, thresholdUsd: 5, amountUsd: 50, minimumAmountUsd: 10, maxPerDay: 4 }
    });

    await client.billingAutoTopup({ amountUsd: 50 });

    // Filling in `enabled: false` here would switch auto-recharge OFF for anyone
    // who only wanted to change the amount.
    expect(JSON.parse(calls[0]!.body ?? "{}")).toEqual({ amountUsd: 50 });
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
    expect(calls[0]!.headers.get("idempotency-key")).toMatch(idPattern("idempotency"));
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
          sessionId: null,
          // The WS2 cost-attribution tag: sent on every row, `null` for
          // org-level entries like this top-up. Previously undeclared.
          workspaceId: null,
          description: "admin top-up",
          createdBy: "admin:ops@example.test",
          // Raw Data-API text, NOT ISO-8601 — this column is selected unformatted.
          createdAt: "2026-07-01 00:00:00"
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
