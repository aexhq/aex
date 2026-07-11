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

Use `retry: false` or `{ maxAttempts: 1 }` for one transport attempt.

## Application runs are not retried

The SDK never reruns a whole user scenario after a terminal failure. A failed
run is the product result a user would observe. Reliability belongs below that
boundary, in idempotent transport, checkpointing, and the hosted runtime.

When your application deliberately repeats a create or message mutation, reuse
its idempotency key:

```ts
const result = await aex.start({
  model,
  message: "Write the report.",
  idempotencyKey: "report-2026-07-10",
  apiKeys
});
```

`Aex.start` derives a stable message key from the create key, so repeating the
same call cannot create a second billable run. A changed request under the same
key fails with an idempotency conflict.

For an explicit user-driven retry on an existing session, call
`session.messages.replayLast()` after applying your own policy. It reuses the
last message key by default.

## Throttling

After eligible transport attempts are exhausted, the SDK throws
`AexRateLimitError`. Use `isRateLimited(error)` and inspect `status`,
`attempts`, `retryAfterMs`, `source`, and `providerFault`. Error bodies and
secrets are redacted.
