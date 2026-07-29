---
title: Retries and throttling
---

# Retries and throttling

The SDK retries transport failures only when repeating the HTTP request is
provably safe:

- Safe reads (`GET`, `HEAD`, and `OPTIONS`) may retry.
- A mutation may retry only when it carries a stable `Idempotency-Key`.
- A `POST`, `PATCH`, `PUT`, or `DELETE` without that key is attempted once.

Eligible requests retry network failures and HTTP `429`, `500`, `502`, `503`,
`504`, and `529` with bounded exponential backoff and jitter. `Retry-After` is
honored. Validation, authentication, not-found, and conflict responses fail
immediately.

```ts
const aex = new Aex({
  apiKey: process.env.AEX_API_KEY!,
  retry: {
    maxAttempts: 4,
    initialDelayMs: 500,
    maxDelayMs: 20_000,
    maxElapsedMs: 120_000
  }
});
```

Use `retry: false` or `{ maxAttempts: 1 }` for one general transport attempt.
The client policy applies both to hosted API requests and to direct
object-storage PUTs performed while publishing file, skill, tool, and
instruction assets.

`session.files.fetch()` is deliberately lower level: it returns the raw
`Response` from a signed file URL and does not retry that transfer. The bounded
`session.files.read()` and `session.files.download()` helpers may repeat a safe
file GET once only when their per-attempt transfer timeout expires. None of
these transfer policies repeat an agent run.

## Application runs are not retried

The SDK never reruns a whole user scenario after a terminal failure. A failed
run is the product result a user would observe. Reliability belongs below that
boundary, in idempotent transport, checkpointing, and the hosted runtime.

## Idempotency is handled for you

There is no idempotency key on the SDK or CLI surface. You do not generate one,
pass one, or store one.

It still exists on the wire, and it is load-bearing. The API has a 29-second
ceiling, so a submit can succeed server-side while its response never reaches
you. The SDK mints a key per logical mutation and replays **that same key**
across its own automatic retries, so the retried attempt is recognised as the
original and de-duplicated — instead of creating a second session, a second
container, and a second bill. `Aex.start` additionally derives the first-message
key from the create key, so both halves of one start are covered.

A retry loop you write yourself is **not** covered by that guarantee: each pass
is a new logical mutation carrying a new key, and each one bills. If a call
still fails once the SDK's own retries are exhausted, treat it as failed rather
than reissuing it blind, then reconcile with `aex.sessions.list()` and delete
anything duplicated. The blast radius is bounded — a duplicate session hits its
`maxIdleTtl`, checkpoints itself, and is listable and deletable.

To repeat a turn on an existing session deliberately, call
`session.messages.replayLast()`. It re-presents the ORIGINAL key, so a server
that already recorded that turn de-duplicates it rather than billing another.
`session.messages.send(...)` is how you ask for a genuinely new turn.

## Throttling

After eligible transport attempts are exhausted, the SDK throws
`AexRateLimitError`. Use `isRateLimited(error)` and inspect `status`,
`attempts`, `retryAfterMs`, `source`, and `providerFault`. Error bodies are
scanned for secret shapes CLIENT-SIDE, inside your own process, before an
`AexError` carries them (`redactSecrets`, exported from the SDK). Nothing is sent
anywhere to do it, and it does not apply to your session's content — only to the
error objects this SDK constructs.

Provider failures are machine-readable on failed detail records as
`session.providerFault` and on terminal events as
`RUN_ERROR.data.providerFault`:

```ts
const fault = result.session.providerFault;
if (fault?.kind === "rate_limit" || fault?.kind === "overloaded") {
  // Apply an application-level replay policy if appropriate.
}
```

The canonical object has exact fields `provider?`, `kind`, `status?`,
`retryAfterMs?`, and `message?`. Known kinds are `rate_limit`, `overloaded`,
`quota_exceeded`, `unavailable`, and `provider_error`. The SDK treats only the
first four as throttle signals. A valid future kind is preserved but is not a
throttle until a later SDK explicitly recognizes it; status codes and prose do
not override the kind.

For sessions created by older runtimes that do not have the field, the SDK has
a temporary compatibility bridge for the exact historical
`transient-provider` failure class and exact historical terminal templates.
It does not scan arbitrary error prose. With `debug` enabled, each bridge use
emits one local line with code `legacy_provider_fault_fallback`; the line
contains only the mapped kind and `source=session` — nothing else is included, so
there is nothing in it to mask.

The bridge is eligible for removal only in a separate major release, after at
least two minor releases and 90 days with zero observed fallback use. That
earliest review is 2026-10-20; removal is not part of this contract change.
