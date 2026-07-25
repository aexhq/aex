---
title: Authentication
---

# Authentication

Every SDK and CLI call authenticates with an **aex API key**: a bearer
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

Pass the token to the SDK constructor, the CLI's `--api-key` flag, or persist
it once with `aex login`:

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);
```

```bash
npx aex whoami --api-key "$AEX_API_KEY"

# or persist it once, then omit --api-key on later commands:
npx aex login --api-key "$AEX_API_KEY"
npx aex whoami
```

(`new Aex(apiKey)` is the canonical constructor; the equivalent options form is
documented once in [Credentials](credentials.md).)

The token travels as a standard `Authorization: Bearer` header. Treat it like
any other secret: keep it in environment variables or a secret manager, never
in session config, prompts, or committed files.

## Plane routing and the `baseUrl` guard

An aex API key is **self-describing**: it embeds the plane (`dev` / `prd`) and
region it was minted for. The constructor parses the key and routes accordingly,
with **zero network**:

- **Omit `baseUrl`** and the key routes to its canonical hosted API plane:
  `prd` keys route to `https://api.aex.dev`, and `dev` keys route to
  `https://dev-api.aex.dev`.
- **Supply a `baseUrl` whose plane disagrees with the key** (e.g. a `dev` key
  pointed at `https://api.aex.dev`) and the constructor throws
  `CredentialValidationError` **before any request** — you no longer discover the
  mismatch as a late `401 token_invalid` after a full round-trip.

```ts
// prd key → routes to https://api.aex.dev automatically:
const prd = new Aex(process.env.AEX_PRD_KEY!);

// dev key → routes to https://dev-api.aex.dev automatically:
const dev = new Aex(process.env.AEX_DEV_KEY!);
```

## Scopes

A token carries a set of **scopes**; every API route requires one, and a token
that lacks the route's scope fails with `403 insufficient_scope` naming the
missing scope. The customer-grantable scopes:

| Scope | Grants |
| --- | --- |
| `sessions:read` | Read sessions, their events and event archives, and webhook delivery ledgers. |
| `sessions:write` | Create sessions, send messages, suspend/resume, redeliver webhooks, reveal the webhook signing secret. |
| `sessions:cancel` | Cancel a session/run. |
| `sessions:delete` | Delete a session and its record. |
| `files:read` | Read captured session files and reusable workspace-file metadata. |
| `files:write` / `files:delete` | Publish and delete reusable workspace-file resources. |
| `assets:write` | Upload content-addressed bytes before publishing a typed workspace resource. |
| `assets:delete` | Delete unreferenced content-addressed bytes. |
| `skills:read` / `skills:write` / `skills:delete` | Administer workspace skills. |
| `tools:read` / `tools:write` / `tools:delete` | Administer workspace custom tools. |
| `instructions:read` / `instructions:write` / `instructions:delete` | Administer workspace instructions. |
| `secrets:read` | List workspace secrets and read their metadata (never values). |
| `secrets:write` | Create, rotate, and delete workspace secrets. |
| `mcp:read` | List and read workspace MCP server configurations. |
| `mcp:write` | Create workspace MCP server configurations. |
| `mcp:delete` | Delete workspace MCP server configurations. |
| `billing:read` | Read billing state and create hosted billing sessions. |
| `workspaces:delete` | Owner self-service workspace hard-erase. |

A typical automation token carries `sessions:read`, `sessions:write`, and
`files:read`, plus the resource scopes it publishes. Grant the rest only where the workload needs them — a read-only
reporting token, for example, needs no `sessions:write`.

## Introspection: `whoami`

`aex.whoami()` (CLI: `aex whoami`) validates the token and returns the
workspace id, the token's scopes, and — on current platform deployments — a
`limits` object with the workspace's effective admission caps
(`maxConcurrentSessions`, `submitRatePerMinute`, `spendCapUsd`, `monthSpendUsd`,
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

Model access needs no provider key of your own — the platform's managed gateway
key serves every run. Runtime secrets your code and MCP servers need are a
separate, per-call surface — see [Credentials](credentials.md) and
[Secrets](secrets.md).
