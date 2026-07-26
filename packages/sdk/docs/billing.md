---
title: Billing & webhook signing secret
---

# Billing & webhook signing secret

Workspace-level billing and webhook verification calls are token-scoped like
every other client call — the workspace is derived server-side from the API key.

The billing reads — `aex.billing()`, `aex.billingLedger()`, and the CLI
`aex billing` (and its `ledger` sub-verb) — require the **`billing:read`**
scope; a token without it fails with `403 insufficient_scope` (see
[Errors](errors.md)). This is why the [Quickstart](quickstart.md) mints
`billing:read` alongside `sessions:read` / `sessions:write` / `files:read`.

## How you are charged

There are no plans. Every workspace gets a **free monthly allowance in each
metered dimension**, which resets at the start of each UTC month; usage past an
allowance is paid for out of **prepaid credit**. A card is optional — without
one, work stops when the model-usage allowance runs out.

## Read the billing summary

`aex.billing()` returns the prepaid balance, current-month spend, the spend cap
enforced on new sessions, this period's allowances, the auto-recharge settings
and the saved card:

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);
const billing = await aex.billing();

console.log(billing.balanceUsd, billing.monthSpendUsd, billing.spendCapUsd);
for (const allowance of billing.allowances) {
  console.log(`${allowance.label}: ${allowance.remaining} of ${allowance.quota} ${allowance.unit} left`);
}
```

`billing.admissionState` is `"free"` (no card), `"carded_manual"` or
`"carded_auto"` — card presence is the only lever. The returned
`BillingSummary` is additive-tolerant: fields a newer deployment reports that
this SDK version does not know yet pass through on the object instead of being
rejected.

CLI equivalent:

```bash
aex billing            # balance / month spend / spend cap / allowances
aex billing --json     # the raw wire body for scripting
```

## Buy prepaid credit

`aex.billingTopup({ amountUsd })` creates a hosted checkout session. Open the
returned URL in a browser; the same flow saves the card on first use, and the
balance moves once the charge settles.

```ts
const { url } = await aex.billingTopup({
  amountUsd: 25
}, {
  idempotencyKey: crypto.randomUUID()
});
console.log(url);
```

Amounts below the published minimum are refused — read it from
`billing.autoTopup.minimumAmountUsd` rather than hard-coding a figure.

The optional second argument identifies the mutation and is sent only as the
`Idempotency-Key` header. When omitted, the SDK generates one before transport
retries begin and reuses it for every attempt.

## Auto-recharge

`aex.billingAutoTopup(...)` turns automatic top-ups on and sets the trigger and
the amount. It is **off by default** and stays off until you ask for it: a saved
card is not consent to a standing charge. Enabling it requires a saved card, and
`thresholdUsd` must stay strictly below `amountUsd`.

```ts
const { autoTopup } = await aex.billingAutoTopup({
  enabled: true,
  thresholdUsd: 5,
  amountUsd: 20
});
console.log(autoTopup.enabled, autoTopup.maxPerDay);
```

Omitted fields keep their stored value. `autoTopup.maxPerDay` is the cap on
successful automatic recharges in a rolling 24 hours — a runaway workload trips
it rather than draining the card.

`aex.billingPortal()` creates a hosted billing portal session for the workspace:

```ts
const { url } = await aex.billingPortal(
  { returnUrl: "https://aex.dev/billing" },
  { idempotencyKey: crypto.randomUUID() }
);
console.log(url);
```

CLI equivalents:

```bash
aex billing topup 25 --idempotency-key "$KEY"
aex billing autotopup --enable --threshold 5 --amount 20
aex billing portal --idempotency-key "$KEY"
```

## Read the credit ledger

`aex.billingLedger({ limit })` returns recent credit-ledger rows, newest first —
top-ups, adjustments, and run charges with signed `amountUsd` (credits positive,
charges negative). The ledger holds cash only: free allowances are quantities and
never appear as ledger rows. `limit` is clamped server-side to [1, 100] (default
25); the read is not cursor-paged.

```ts
const { entries } = await aex.billingLedger({ limit: 50 });
for (const entry of entries) {
  console.log(entry.createdAt, entry.entryType, entry.amountUsd);
}
```

CLI equivalent:

```bash
aex billing ledger --limit 50   # JSON rows, newest first
```

## Reveal the webhook signing secret

Session webhooks are signed Standard-Webhooks style with a per-workspace secret.
`aex.webhookSigningSecret()` reveals it (creating one on first use) as the
`whsec_<base64>` string that `verifyAexWebhook` takes as `secret`:

```ts
import { Aex, verifyAexWebhook } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);
const { whsec } = await aex.webhookSigningSecret();

// In your webhook receiver:
const verified = await verifyAexWebhook({
  rawBody,          // the exact request body bytes as a string
  headers,          // the inbound request headers
  secret: whsec
});
```

Repeat calls return the SAME value — the hosted API does not rotate the
signing secret. Treat the reveal as sensitive: store it in your secret manager
and never log it.

CLI equivalent (prints the bare `whsec_...` string, pipeable into a secret
store; the reveal never goes to stderr or debug traces):

```bash
aex webhooks secret
```
