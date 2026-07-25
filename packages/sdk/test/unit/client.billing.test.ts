import { describe, expect, it } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
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

describe("aex.billing", () => {
  it("GETs /api/billing and returns the typed summary", async () => {
    // Every key the handler sends, unconditionally. `accountType` and
    // `pastDueAt` were absent from this fixture AND from `BillingSummary` while
    // the server sent both on every call — the index signature that used to sit
    // on that interface is what made the omission invisible.
    const summary = {
      balanceUsd: 12.5,
      monthSpendUsd: 0.42,
      spendCapUsd: 50,
      planKey: "free",
      subscriptionStatus: "none",
      paymentMethodStatus: "none",
      accountType: "standard",
      pastDueAt: null
    } as const;
    const { client, calls } = billingClient(summary);

    const result = await client.billing();

    expect(result).toEqual(summary);
    expect(calls).toHaveLength(1);
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/billing");
    expect(url.search).toBe("");
    expect(calls[0]!.method).toBe("GET");
  });

  it("passes an undeclared server field through the read without rejecting it", async () => {
    // `getBilling` does not parse; it hands the decoded body back. So an
    // undeclared key still arrives at RUN TIME — what changed is that
    // `BillingSummary` no longer DECLARES that it will, because
    // `[key: string]: unknown` made every undeclared field structurally legal
    // and so made `accountType` / `pastDueAt` impossible to notice missing.
    // Reaching one now requires an explicit widening, which is the point.
    const summary = {
      balanceUsd: 1,
      monthSpendUsd: 0,
      spendCapUsd: 10,
      planKey: "free",
      subscriptionStatus: "none",
      paymentMethodStatus: "active",
      accountType: "internal",
      pastDueAt: "2026-07-01 12:00:00",
      plan: { name: "future-field" }
    } as const;
    const { client } = billingClient(summary);

    const result = await client.billing();

    expect(result).toEqual(summary);
    expect(result.paymentMethodStatus).toBe("active");
    expect(result.accountType).toBe("internal");
    // The raw Data-API rendering, NOT ISO-8601 — the same concept on
    // `whoami.limits.pastDueAt` carries a `T` and a `Z`.
    expect(result.pastDueAt).toBe("2026-07-01 12:00:00");
    // The double cast IS the proof: with the index signature gone, reaching an
    // undeclared key is no longer something the type quietly allows.
    expect((result as unknown as Record<string, unknown>)["plan"]).toEqual({ name: "future-field" });
  });
});

// `aex.billingCheckout` is gone with POST /api/billing/checkout, which the
// plan-catalog demolition removed server-side. The retry-identity behaviour it
// used to cover is real and still worth pinning, so it moves to the sibling
// billing mutation that survives rather than being deleted with the route.
describe("aex.billingPortal retry identity", () => {
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

    await client.billingPortal({ returnUrl: "https://aex.dev/billing" });

    expect(identities).toHaveLength(2);
    expect(identities[0]).toMatch(/^aex-idem-/);
    expect(identities[1]).toBe(identities[0]);
    expect(bodies).toEqual([
      { returnUrl: "https://aex.dev/billing" },
      { returnUrl: "https://aex.dev/billing" }
    ]);
  });
});

describe("aex.billingPortal", () => {
  it("POSTs /api/billing/portal and returns the hosted URL", async () => {
    const { client, calls } = billingClient({ url: "https://billing.stripe.test/session" });

    const result = await client.billingPortal(
      { returnUrl: "https://aex.dev/billing" },
      { idempotencyKey: "portal-key" }
    );

    expect(result).toEqual({ url: "https://billing.stripe.test/session" });
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/billing/portal");
    expect(calls[0]!.method).toBe("POST");
    expect(JSON.parse(calls[0]!.body ?? "{}")).toEqual({ returnUrl: "https://aex.dev/billing" });
    expect(calls[0]!.headers.get("idempotency-key")).toBe("portal-key");
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
