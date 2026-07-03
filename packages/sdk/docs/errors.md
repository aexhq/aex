---
title: Errors
---

# Errors

Every API error is a JSON body with a machine-readable `error` code; most also
carry a human `message` and the self-describing fields named below. The SDK
surfaces non-2xx responses as `AexApiError` (with the parsed body attached) and
throttling as `AexRateLimitError`.

## 401 — authentication

| Code | Meaning |
| --- | --- |
| `unauthorized` | Missing, invalid, or revoked bearer token. |

Check the token value and that it has not been deleted. `aex whoami` is the
cheapest way to validate a credential. See [Authentication](authentication.md).

## 403 — authorization

| Code | Meaning |
| --- | --- |
| `insufficient_scope` | The token is valid but lacks the route's required scope. The body's `requiredScope` field names the missing scope. |
| `unknown_workspace` | The token does not route to a known workspace. |
| `forbidden` | The authenticated workspace does not own the addressed resource. |

```json
{ "error": "insufficient_scope", "requiredScope": "runs:write" }
```

## 400 — validation

| Code | Meaning |
| --- | --- |
| `bad_request` | Missing or unparseable request body. |
| `invalid_submission` | The submission failed shape validation; `message` names the offending field. |
| `missing_provider_key` | The submission names a provider but carries no BYOK key for it (`apiKeys[provider]`). |
| `malformed_token` | The bearer value is not a structurally valid aex token. |

400s are permanent for that request — fix the input rather than retrying.
The SDK's client-side validation (`RunConfigValidationError`) catches most of
these before the request is sent.

## 402 — payment required

Two distinct submit gates return 402; both bodies are self-describing.

**`insufficient_balance`** — the workspace prepaid balance is at or below the
effective submit floor. Top up the balance or bind a payment method.

```json
{
  "error": "insufficient_balance",
  "message": "Workspace balance is depleted; top up your prepaid balance or bind a payment method to submit runs.",
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
the workspace's concurrent-run cap. Wait for a run to finish, or contact
support to raise the cap.

```json
{
  "error": "workspace_concurrency_exceeded",
  "message": "Workspace concurrency limit reached: 50 live runs at the cap of 50. Wait for a run to finish, or contact support to raise your workspace limit.",
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
does not distinguish the two).

## 5xx — server errors

| Code | Meaning |
| --- | --- |
| `internal_error` (500) | Unexpected server fault. Retry with backoff; report persistent cases. |
| `db_resuming` (503) | The database tier is resuming from idle. Transient — retry. |

The SDK retries transient failures automatically: HTTP `429`, `5xx`, `529`, and
network errors get bounded exponential backoff with full jitter, honoring any
`Retry-After` header. Tune or disable this with the client `retry` option; use
`isRateLimited(err)` / `AexRateLimitError` to handle persistent throttling
without parsing raw bodies. Idempotent submit retries are safe — the SDK
attaches a stable idempotency key to billable session create/send requests, so
a retried request never double-submits.
