---
title: Webhooks
---

# Webhooks

aex can notify your endpoint when a run finishes. Webhooks are **per-run**: you
pass a callback URL with the submission, and the platform delivers exactly one
`run.finished` event to it when the run reaches its terminal state.

> **Availability note:** terminal webhook delivery for session-based runs
> requires the next platform deploy — on the current hosted plane a session
> run's webhook may not fire. This note will be removed once the deploy lands.

## Register a callback

```ts
import { Aex, Models } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);

const session = await aex.openSession({
  model: Models.CLAUDE_HAIKU_4_5,
  webhook: { url: "https://hooks.example.com/aex" },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

```bash
aex run \
  --api-key "$AEX_API_KEY" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write the report." \
  --webhook https://hooks.example.com/aex
```

The URL must be `https`. The callback URL is an operational concern, not part
of the submission fingerprint: retrying the same submission with the same
idempotency key but a different callback URL never conflicts.

## What gets delivered

One POST carrying the terminal `run.finished` event, sent at the
settle-consistent barrier — when you receive it, the run record is already
terminal and its outputs are complete and readable. The body is built once at
settle, frozen, and re-sent byte-identical on every retry or manual redelivery:

```json
{
  "specversion": "1.0",
  "id": "<delivery id — matches the webhook-id header>",
  "source": "aex",
  "type": "run.finished",
  "subject": "<the run id>",
  "time": "2026-07-02T12:34:56.000Z",
  "data": {
    "runId": "<the run id>",
    "status": "succeeded",
    "terminalAt": "2026-07-02T12:34:56.000Z",
    "reason": null,
    "failureClass": null,
    "costTelemetry": { "billedCostUsd": 0.42 }
  }
}
```

`reason` / `failureClass` carry the failure detail on non-success terminals;
`costTelemetry` is present when the billed cost is known.

## Verify deliveries

Deliveries are signed [Standard Webhooks](https://www.standardwebhooks.com/)
style: HMAC-SHA256 over `` `${webhook-id}.${webhook-timestamp}.${rawBody}` ``,
sent in three headers — `webhook-id` (stable across retries; your dedupe key),
`webhook-timestamp` (unix seconds), and `webhook-signature` (a space-delimited
list of `v1,<base64>` entries).

The signing key is a per-workspace secret. Reveal it (it is created on first
use) with either surface:

```bash
aex webhooks secret --api-key "$AEX_API_KEY"   # prints whsec_...
```

```ts
const { whsec } = await aex.webhookSigningSecret();
```

Verify inbound requests with the exported `verifyAexWebhook` — pure Web Crypto,
identical under Bun and Node, and interoperable with the `standardwebhooks`
reference library:

```ts
import { verifyAexWebhook } from "@aexhq/sdk";

// In your HTTP handler — use the EXACT raw body bytes, never re-serialize.
const ok = await verifyAexWebhook({
  rawBody,
  headers: request.headers,
  secret: process.env.AEX_WEBHOOK_SECRET! // the whsec_... value
});
if (!ok) return new Response("bad signature", { status: 401 });
```

`verifyAexWebhook` rejects stale timestamps (default tolerance 300 seconds) and
compares signatures constant-time. It accepts every `v1` entry in the signature
list, so it is already rotation-ready — but **server-side rotation of the
signing secret is not available yet**: `aex webhooks secret` reveals or creates
the one workspace secret and there is no rotate endpoint today. If your secret
is compromised, contact <support@aex.dev>.

## Delivery ledger and redelivery

Each run keeps a delivery ledger — attempts, last status code, and last error:

```ts
const deliveries = await session.webhooks().list();
await session.webhooks().redeliver(deliveries[0]!.id);
```

```bash
aex deliveries <session-id> --api-key "$AEX_API_KEY"
```

Redelivery re-sends the frozen payload with the **same** `webhook-id`, so a
consumer that dedupes on `webhook-id` handles retries, redeliveries, and
at-least-once delivery uniformly. An empty ledger means the run carried no
`webhook` or has not reached a terminal state yet.

For the terminal-event mechanics behind delivery timing, see
[Events](events.md).
