---
title: Authentication
---

# Authentication

Every SDK and CLI call authenticates with an **aex API token**: a bearer
credential that is **workspace-scoped** — the workspace is derived server-side
from the token, so there is no workspace parameter anywhere in the API. A
request either carries a valid token for a workspace or it is rejected with
`401 unauthorized` (see [Errors](errors.md)).

## Getting a token (invite-only beta)

aex is currently in **invite-only beta**. Workspaces and their initial API
tokens are provisioned by the aex team — contact <support@aex.dev> for beta
access. Once your workspace exists, you can create and delete additional tokens
in the dashboard at <https://aex.dev>.

## Using a token

Pass the token to the SDK constructor, the CLI's `--api-token` flag, or persist
it once with `aex login`:

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_TOKEN!); // preferred
// also accepted: new Aex({ apiKey: ... }) and the alias new Aex({ apiToken: ... })
```

```bash
aex whoami --api-token "$AEX_API_TOKEN"

# or persist it once, then omit --api-token on later commands:
aex login --api-token "$AEX_API_TOKEN"
aex whoami
```

The token travels as a standard `Authorization: Bearer` header. Treat it like
any other secret: keep it in environment variables or a secret manager, never
in run config, prompts, or committed files.

## Scopes

A token carries a set of **scopes**; every API route requires one, and a token
that lacks the route's scope fails with `403 insufficient_scope` naming the
missing scope. The customer-grantable scopes:

| Scope | Grants |
| --- | --- |
| `runs:read` | Read sessions/runs, their events and event archives, and webhook delivery ledgers. |
| `runs:write` | Open sessions, send turns, submit runs, suspend/resume, redeliver webhooks, reveal the webhook signing secret. |
| `runs:cancel` | Cancel a session/run. |
| `runs:delete` | Delete a session/run record. |
| `outputs:read` | List, read, download, and mint links for captured outputs. |
| `files:write` | Upload asset bytes (files, skills, AGENTS.md) staged for a run. |
| `files:delete` | Delete workspace asset-store entries. |
| `secrets:read` | List workspace secrets and read their metadata (never values). |
| `secrets:write` | Create, rotate, and delete workspace secrets. |
| `billing:read` | Read billing state and create hosted billing sessions. |
| `workspaces:delete` | Owner self-service workspace hard-erase. |

A typical automation token carries `runs:read`, `runs:write`, and
`outputs:read`. Grant the rest only where the workload needs them — a read-only
reporting token, for example, needs no `runs:write`.

## Introspection: `whoami`

`aex.whoami()` (CLI: `aex whoami`) validates the token and returns the
workspace id, the token's scopes, and — on current platform deployments — a
`limits` object with the workspace's effective admission caps
(`maxConcurrentRuns`, `submitRatePerMinute`, `spendCapUsd`, `monthSpendUsd`,
`balanceUsd`, `balanceGraceFloorUsd`, `paymentMethodStatus`), resolved by the
same code the submit gates enforce. Use it as a cheap credential check and to
anticipate `429`/`402` rejections before submitting — see
[Limits & quotas](limits-and-quotas.md) and [Errors](errors.md).

## Rotation and revocation

- **Rotate** by creating a new token in the dashboard, switching your
  deployment to it, then deleting the old one. Tokens are independent — both
  stay valid during the switch, so rotation needs no downtime.
- **Revoke** by deleting the token in the dashboard; deletion takes effect
  immediately (allow up to a minute for in-flight caches to expire).
- Token create/delete are rate-limited per workspace (see
  [Limits & quotas](limits-and-quotas.md)).
- If you cannot reach the dashboard (or suspect workspace-level compromise),
  contact <support@aex.dev>.

Provider keys (`apiKeys`) and runtime secrets are a separate, per-call surface —
see [Credentials](credentials.md) and [Secrets](secrets.md).
