---
title: Billing & webhook signing secret
---

# Billing & webhook signing secret

Workspace-level billing, subscription, and webhook verification calls are
token-scoped like every other client call — the workspace is derived
server-side from the API key.

## Read the billing summary

`aex.billing()` returns the workspace's prepaid balance, current-month spend,
and the spend cap enforced on new runs, plus plan fields:

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);
const billing = await aex.billing();
console.log(billing.balanceUsd, billing.monthSpendUsd, billing.spendCapUsd);
```

The returned `BillingSummary` is additive-tolerant: fields a newer deployment
reports that this SDK version does not know yet pass through on the object
instead of being rejected.

CLI equivalent:

```bash
aex billing            # human-readable balance / month spend / spend cap
aex billing --json     # the raw wire body for scripting
```

## Manage the subscription

`aex.billingCheckout({ planKey })` creates a hosted checkout session for a paid
plan. Open the returned URL in a browser; the workspace plan changes after
checkout completes and the hosted API confirms the subscription.

```ts
const { url } = await aex.billingCheckout({
  planKey: "pro",
  idempotencyKey: crypto.randomUUID()
});
console.log(url);
```

`aex.billingPortal()` creates a hosted billing portal session for the workspace:

```ts
const { url } = await aex.billingPortal({ returnUrl: "https://aex.dev/billing" });
console.log(url);
```

CLI equivalents:

```bash
aex billing upgrade pro
aex billing portal
```

## Read the credit ledger

`aex.billingLedger({ limit })` returns recent credit-ledger rows, newest first —
allowance grants, adjustments, and run charges with signed `amountUsd` (credits
positive, charges negative). `limit` is clamped server-side to [1, 100] (default
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

Run webhooks are signed Standard-Webhooks style with a per-workspace secret.
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
