---
title: Webhooks
---

# Webhooks

aex can notify your endpoint whenever a run finishes. Register the callback on a
session and the platform delivers one run-scoped event after each run finalizes:
`run.finished` for AG-UI `RUN_FINISHED`, or `run.error` for `RUN_ERROR`. Sending
another message on the same session produces another independently identifiable
delivery.

## Register a callback

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);

const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5",
  webhook: { url: "https://hooks.example.com/aex" },
});
```

```bash
aex start \
  --api-key "$AEX_API_KEY" \
  --model anthropic/claude-haiku-4-5 \
  --prompt "Write the report." \
  --webhook https://hooks.example.com/aex
```

The URL must be `https`. The callback URL is an operational concern, not part
of the submission fingerprint: retrying the same submission with the same
idempotency key but a different callback URL never conflicts.

## What gets delivered

Each finalized run produces one frozen CloudEvents envelope. A successful
`run.finished` delivery is sent only after its checkpoint and session-file
revision are committed and the session read model is consistent. A `run.error`
delivery represents the corresponding finalized `RUN_ERROR`; `checkpoint` is
present only when that run committed one. Retries and manual redelivery resend
the same bytes and `webhook-id`.

```json
{
  "specversion": "1.0",
  "id": "whd_run_01J...",
  "source": "aex",
  "type": "run.finished",
  "subject": "run_01J...",
  "time": "2026-07-02T12:34:56.000Z",
  "data": {
    "sessionId": "session_01J...",
    "runId": "run_01J...",
    "turnSeq": 3,
    "outcome": "succeeded",
    "terminalAt": "2026-07-02T12:34:56.000Z",
    "checkpoint": {
      "checkpointId": "checkpoint_01J...",
      "runId": "run_01J...",
      "turnSeq": 3,
      "committedAt": "2026-07-02T12:34:55.900Z",
      "throughSeq": 4097
    },
    "reason": null,
    "failureClass": null,
    "costTelemetry": { "billedCostUsd": 0.42 }
  }
}
```

`subject` and `data.runId` identify the run; `data.sessionId` identifies the
resumable thread. `outcome` is the terminal run outcome. `reason` and
`failureClass` carry failure detail for `run.error`; `checkpoint` and
`costTelemetry` are included when available.

## Verify deliveries

Deliveries are signed [Standard Webhooks](https://www.standardwebhooks.com/)
style: HMAC-SHA256 over `` `${webhook-id}.${webhook-timestamp}.${rawBody}` ``,
sent in three headers — `webhook-id` (stable for that run across retries; your dedupe key),
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

Each session keeps a ledger with one row per finalized run, including its
`runId`, `turnSeq`, event type, attempts, last status code, and last error:

```ts
const deliveries = await session.webhooks.list();
await session.webhooks.redeliver(deliveries[0]!.id);
```

```bash
aex deliveries <session-id> --api-key "$AEX_API_KEY"
```

Redelivery re-sends that run's frozen payload with the **same** `webhook-id`, so
a consumer that dedupes on `webhook-id` handles retries, redeliveries, and
at-least-once delivery uniformly. Different runs have different delivery IDs.
An empty ledger means the session carried no `webhook` or no run has finalized
yet.

For the terminal-event mechanics behind delivery timing, see
[Events](events.md).
