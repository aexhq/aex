---
title: Errors
---

# Errors

Every API error is a JSON body with a machine-readable `error` code; most also
carry a human `message` and the self-describing fields named below.

## Typed errors in the SDK

The SDK maps every non-2xx response through one factory to a typed exception. All
inherit `AexApiError`, which carries the HTTP `status`, the redacted parsed
`body`, the server's stable `apiCode` (a machine-branchable identity distinct
from the human `message`), and a `requestId` for support correlation. The
factory dispatches to a subclass by code/status:

| Class | Fires for | Extra fields |
| --- | --- | --- |
| `AexAuthError` | `401` / `403` (unauthorized, forbidden, insufficient_scope, token_invalid/revoked/expired, malformed_token) | `requiredScope` (on `insufficient_scope`) |
| `AexIdempotencyConflictError` | `409` `idempotency_conflict` | — |
| `AexNotFoundError` | `404` `not_found` | — |
| `AexRateLimitError` | `429` (rate_limited, workspace_concurrency_exceeded, workspace_submit_rate_exceeded) | `retryAfterMs` (when advertised) |
| `AexApiError` (base) | every other stable code (session_busy, checkpoint_not_available, session_not_terminal, session_terminal, event_archive_too_large, event_archive_deadline_exceeded, workspace_inactive, insufficient_balance, workspace_spend_cap_exceeded, upstream_error, internal_error, …) | — |

Branch with the exported guards instead of parsing bodies or matching status
codes: `isAuthError`, `isInsufficientScope`, `isIdempotencyConflict`,
`isNotFound`, and `isRateLimited`.

```ts
import { isInsufficientScope, isIdempotencyConflict } from "@aexhq/sdk";

try {
  await aex.start(config);
} catch (err) {
  if (isInsufficientScope(err)) {
    // err.requiredScope names the missing scope; mint a key that includes it.
  } else if (isIdempotencyConflict(err)) {
    // same key, different body — use a fresh key or resend the byte-identical body.
  }
}
```

The **stable** `apiCode` set the SDK types and dispatches on is: `unauthorized`,
`forbidden`, `insufficient_scope`, `token_invalid`, `token_revoked`,
`token_expired`, `malformed_token`, `not_found`, `idempotency_conflict`,
`session_busy`, `checkpoint_not_available`, `session_not_terminal`, `session_terminal`,
`event_archive_too_large`, `event_archive_deadline_exceeded`, `unknown_workspace`,
`workspace_inactive`, `workspace_concurrency_exceeded`, `workspace_submit_rate_exceeded`,
`workspace_spend_cap_exceeded`, `insufficient_balance`, `rate_limited`,
`upstream_error`, `internal_error`. A code outside this set (e.g. a validation
`error` a route reports) still surfaces as an `AexApiError` with the `status` and
`body.error` preserved; `apiCode` is then `undefined`.

Transport failures with no HTTP response (DNS, connection refused, TLS reset)
surface as `AexNetworkError`; client-side config validation surfaces as
`SessionConfigValidationError` (`err.code === "SESSION_CONFIG_INVALID"`) before any
request is sent. Its stable machine-readable payload is exactly
`err.details = { field }`, where `field` is the rejected public option path such
as `overrides.timeout`. Branch on `details.field`; the human `message` may change
and never includes the rejected value.

## 401 — authentication

| Code | Meaning |
| --- | --- |
| `unauthorized` | Missing or empty bearer token. |
| `token_invalid` | Bearer token is structurally valid but does not match a live credential, has a bad signature, or belongs to an inactive workspace. |
| `token_revoked` | Bearer token matched a revoked credential. |
| `token_expired` | Short-lived internal writer token is past its expiry. |

Check the token value and that it has not been deleted or revoked. `aex whoami`
is the cheapest way to validate a credential. See
[Authentication](authentication.md).

## 403 — authorization

| Code | Meaning |
| --- | --- |
| `insufficient_scope` | The token is valid but lacks the route's required scope. The body's `requiredScope` field names the missing scope. |
| `unknown_workspace` | The token does not route to a known workspace. |
| `forbidden` | The authenticated workspace does not own the addressed resource. |

```json
{ "error": "insufficient_scope", "requiredScope": "sessions:write" }
```

## 400 — validation

| Code | Meaning |
| --- | --- |
| `bad_request` | Missing or unparseable request body. |
| `invalid_submission` | The submission failed shape validation; `message` names the offending field. |
| `missing_provider_key` | The submission names a provider but carries no BYOK key for it (`apiKeys[provider]`). |
| `malformed_token` | The bearer value is not a structurally valid aex token. |

400s are permanent for that request — fix the input rather than retrying.
The SDK's client-side validation (`SessionConfigValidationError`) catches most of
these before the request is sent.

## 402 — payment required

Two distinct submit gates return 402; both bodies are self-describing.

**`insufficient_balance`** — the workspace prepaid balance is at or below the
effective submit floor. Top up the balance or bind a payment method.

```json
{
  "error": "insufficient_balance",
  "message": "Workspace balance is depleted; top up your prepaid balance or bind a payment method to submit sessions.",
  "balanceUsd": 0,
  "balanceGraceFloorUsd": 0,
  "paymentMethodStatus": "none",
  "planKey": "default"
}
```

`balanceGraceFloorUsd` is the payment-method-aware floor the gate compared
against (`paymentMethodStatus: "active"` folds a bounded card overdraft into
it, so the floor can be negative).

**`workspace_spend_cap_exceeded`** — the workspace's monthly spend cap is
reached. The cap resets at the start of the next UTC month; contact support to
raise it.

```json
{
  "error": "workspace_spend_cap_exceeded",
  "message": "Monthly spend cap of $250 reached ($251.13 accrued this month). The cap resets at the start of the next UTC month; contact support to raise it.",
  "capUsd": 250,
  "accruedUsd": 251.13
}
```

## 429 — rate limits

**`workspace_concurrency_exceeded`** — admitting one more live run would exceed
the workspace's concurrent-run cap. Wait for a session to finish, or contact
support to raise the cap.

```json
{
  "error": "workspace_concurrency_exceeded",
  "message": "Workspace concurrency limit reached: 50 live sessions at the cap of 50. Wait for a session to finish, or contact support to raise your workspace limit.",
  "cap": 50,
  "observed": 50
}
```

**`workspace_submit_rate_exceeded`** — too many submits in the current
one-minute window. Retry shortly.

```json
{
  "error": "workspace_submit_rate_exceeded",
  "message": "Submit rate limit of 120/minute exceeded. Retry shortly, or contact support to raise your workspace limit.",
  "perMin": 120,
  "observed": 121
}
```

The `limit`-naming fields (`cap`/`perMin`) and the `observed` window value make
each deny self-describing, so a client can back off proportionally. To
anticipate both 429s and both 402s *before* submitting, read the effective caps
from `aex.whoami().limits` — the values come from the same resolution code the
gates enforce. See [Limits & quotas](limits-and-quotas.md).

## 404 — not found

`not_found`: the id does not exist **or** belongs to another workspace (aex
does not distinguish the two). The SDK raises `AexNotFoundError` (guard:
`isNotFound(err)`).

## 409 — conflicts and inactive workspaces

| Code | Meaning |
| --- | --- |
| `idempotency_conflict` | The `idempotencyKey` was already used with a different request body. The SDK raises `AexIdempotencyConflictError` (guard: `isIdempotencyConflict(err)`). |
| `session_busy` | The session is handling another turn or lifecycle transition. Wait for its current operation to finish. |
| `checkpoint_not_available` | No committed checkpoint exists yet for a checkpoint-backed read such as `session.files.list()`. Wait for the current run to finish. This remains a base `AexApiError`, not an idempotency conflict. |
| `session_not_terminal` | The requested operation requires a terminal session state. |
| `session_terminal` | The session has ended and cannot perform the requested action. |
| `workspace_inactive` | A workspace deletion fence won the admission race, so the workspace no longer accepts new session work. The body carries `workspaceStatus` (normally `deleting`). Use an active workspace. |

For `idempotency_conflict`, use a fresh idempotency key for a genuinely new request, or resubmit
the byte-identical body to replay the original result (a matching retry returns
the existing session rather than conflicting). Note that the SDK validates the
key client-side first: an empty or whitespace-only `idempotencyKey` throws
`SessionConfigValidationError` before the request is sent, as does a key longer
than 255 characters. Never pass `""`.

`workspace_inactive` is not an idempotency conflict and is not transient for
that workspace. It remains a base `AexApiError`; branch on
`err.apiCode === "workspace_inactive"` and inspect `err.body.workspaceStatus`
when the current lifecycle state matters.

## 413/503 — synchronous event archive limits

| Code | Meaning |
| --- | --- |
| `event_archive_too_large` (413) | The history exceeds the synchronous archive's source, candidate, or estimated-output limit. |
| `event_archive_deadline_exceeded` (503) | The synchronous archive could not finish inside its server request budget. |

Both are stable base `AexApiError` codes with `retryable: false`. The SDK does
not retry `session.events.archiveLink()` automatically. Traverse the history
with `session.events.iterate()` instead of repeating the bulk export.

## 5xx — server errors

| Code | Meaning |
| --- | --- |
| `internal_error` (500) | Unexpected server fault. Retry with backoff; report persistent cases. |
| `db_resuming` (503) | The database tier is resuming from idle. Transient — retry. |

The SDK retries transient failures automatically: HTTP `429`, `5xx`, `529`, and
network errors get bounded exponential backoff with full jitter, honoring any
`Retry-After` header. Tune or disable this with the client `retry` option; use
`isRateLimited(err)` / `AexRateLimitError` to handle persistent throttling
without parsing raw bodies. Automatic retries are limited to reads and other
idempotent HTTP methods, or mutations carrying a stable `Idempotency-Key`.
Session create/send requests always carry one stable key across transport
attempts, so a retried request cannot create a second billable run. The SDK
never retries an entire user scenario or a failed application run.
