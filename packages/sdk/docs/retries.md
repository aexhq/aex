---
title: Retries and throttling
---

# Retries and throttling

The SDK ships with built-in transport resilience. Every request it makes to the
aex API is automatically retried on **transient** failures with bounded
exponential backoff and jitter, honoring the server's `Retry-After` header. You
get this by default — no wrapper code — and it is safe to leave on because the
billable submits carry a stable idempotency key, so a retry never creates a
duplicate run.

## What gets retried

Retried automatically:

- HTTP `429` (rate limited)
- HTTP `500`, `502`, `503`, `504` (server hiccups)
- HTTP `529` (upstream provider overloaded)
- Network errors (connection reset, DNS failure, timeout)

Never retried — these fail fast so you see the real problem immediately:

- `400` / `422` (bad request), `401` / `403` (auth), `404` (not found),
  `409` (conflict), and every other non-transient `4xx`.
- A request you aborted yourself (via an `AbortSignal`).

## Tuning or disabling

Pass a `retry` option when you construct the client:

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex({
  apiToken: process.env.AEX_API_TOKEN!,
  retry: {
    maxAttempts: 4,        // total tries incl. the first (default 4)
    initialDelayMs: 500,   // base backoff, doubles per retry (default 500)
    maxDelayMs: 20_000,    // cap on any single wait (default 20s)
    maxElapsedMs: 120_000  // overall wall-clock budget (default 2m)
  }
});
```

Turn it off entirely with `retry: false`, or make a single attempt with
`retry: { maxAttempts: 1 }`.

## Idempotent by construction

Retries — whether the built-in transport retry or your own re-invocation of
`run(...)` — never double-bill. The one-shot `run(...)` and `sessions.run(...)`
derive the turn's idempotency key from the session-create key, so re-invoking
either with the same `idempotencyKey` de-duplicates **both** the session create
and the billable turn server-side:

```ts
// A retried call with the same idempotencyKey resolves to the same run,
// not a second billable one.
const result = await aex.run({
  model: "claude-haiku-4-5",
  message: "Write a short report and save it as a file.",
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! },
  idempotencyKey: "report-2026-07-01"
});
```

## Replaying a throttled turn

When a turn on a live session is interrupted by a throttle, replay the last
message with `session.replayLast()`. It reuses the previous message's idempotency
key by default, so if the original turn actually landed it de-duplicates instead
of billing twice:

```ts
const session = await aex.openSession({
  model: "claude-haiku-4-5",
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

try {
  await session.send("Summarize the attached dataset.").done();
} catch (err) {
  const { isRateLimited } = await import("@aexhq/sdk");
  if (isRateLimited(err)) {
    // Wait out the throttle, then replay the same message.
    await new Promise((r) => setTimeout(r, err.retryAfterMs ?? 2_000));
    await session.replayLast().done();
  } else {
    throw err;
  }
}
```

Pass a fresh key (`session.replayLast({ idempotencyKey: "..." })`) when you
deliberately want a brand-new turn instead of a de-duplicated replay.

## The throttle error

When retries are exhausted on a rate-limit / overloaded status, the SDK throws an
`AexRateLimitError`. It extends `AexApiError`, so existing `catch` sites keep
working, and it carries structured, non-leaky detail:

```ts
import { isRateLimited } from "@aexhq/sdk";

try {
  await aex.run({ /* … */ });
} catch (err) {
  if (isRateLimited(err)) {
    err.status;         // 429 | 503 | 529
    err.attempts;       // how many tries were made
    err.retryAfterMs;   // suggested wait, when the server supplied one
    err.source;         // "api" (aex plane) or "provider" (upstream model)
    err.providerFault;  // upstream fault detail, when the model provider throttled
  }
}
```

The `message` is a fixed summary (e.g. `aex API rate limit reached (HTTP 429)
after 4 attempts; retry after ~2s`) — it never echoes the raw response body,
which stays available, redacted, on `err.body`.

When the throttle originated at the upstream model provider (rather than the aex
API plane), `err.source` is `"provider"` and `err.providerFault` describes it:
its `kind` (`rate_limit` / `overloaded` / `quota_exceeded` / `provider_error`),
the upstream `status`, and a suggested `retryAfterMs`. Use `parseProviderFault`
to read the same shape off a raw fault value yourself.
