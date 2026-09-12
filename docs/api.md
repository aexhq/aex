# Hosted API

Use `@aexhq/sdk` 0.75.1 (Brain SDK 0.24.2) with an issued API key. Brain source is pinned by
full revision in Cargo. Customer keys never authorize direct access to private Brain.

| Methods | Path | Authorization |
| --- | --- | --- |
| GET | `/v1/models?provider=...` | Active key; deployment-allowed models and known capabilities |
| POST, GET | `/v1/sessions` | Active key; lists contain owned sessions only |
| GET, DELETE | `/v1/sessions/{id}` | Account ownership |
| GET | `/v1/sessions/{id}/transcript` | Account ownership |
| GET | `/v1/sessions/{id}/events?after=N` | Ownership; JSON or SSE by Accept header |
| POST | `/v1/sessions/{id}/attachments` | Ownership; bounded upload and required operation key |
| DELETE | `/v1/sessions/{id}/attachments/{attachment}` | Ownership; revokes new reads |
| GET, HEAD | `/v1/attachments/{id}/content?token=...` | Scoped read capability; active account, owned session and unexpired file |
| POST | `/v1/sessions/{id}/messages`, `/cancel`, `/end` | Ownership and operation key |
| POST | `/v1/agentloops`, `/v1/tools` | Active key; bounded account-owned Wasm Components |
| GET | `/v1/agentloops/{id}`, `/v1/tools/{id}` | Account-owned or operator-approved content address |
| POST | `/v1/hosts` | Active account key |
| GET | `/v1/hosts/{id}/commands` | Scoped token and active issuing key/account |
| POST | `/v1/hosts/{id}/results`, `/events` | Scoped token and owned target session |
| GET | `/health/live`, `/health/ready` | No customer data |

Other methods/routes fail closed. Remote Environment URLs, execution
callbacks, provider endpoint overrides and hosted secret/filesystem/network grants are denied.
Application functions run in the customer's live process. Compatible Wasm Tools with no
host resource grants can run in Brain. Committed event cursors and shapes are
preserved. Keepalive comments have no event identity and are inserted only between frames.
Server-error text is redacted while retaining the Brain error code. Bodies and credentials
are never logged.

Host Tools may return Brain Outcomes directly. Structured failures retain code, message, retryable
and details. Tool deadlines produce `timeout`, explicit cancellation produces `cancelled`, and a
possibly dispatched operation without a reliable result produces `unknown`. Each is a failed Tool
result, without automatic replay or a promise of rollback. The existing HTTP and Wasm shapes are
unchanged. See [the Tool return contract](https://aex.dev/brain/docs/guides/write-a-tool#return-values-and-outcomes).

Create claims are account/operation/key scoped. Matching completed creates replay while
ownership exists; differing payloads conflict. Unresolved creates are never dispatched again,
even after restart: operators reconcile the ambiguous result and any inaccessible orphan.
Host registration also returns success only after ownership commits, but has no create replay.

Other mutation keys are account/path scoped and use Brain's bounded replay contract. They
are not an unlimited exactly-once guarantee. Occupied sessions reject concurrent messages;
uncertain turn reservations require drained reconciliation. Revocation does not undo a
previously dispatched effect. Deletion denies access before cleanup, and uncertain cleanup
keeps its tombstone. End a session before deleting it; a premature delete is rejected without
hiding the session. Responses include a server-generated `x-aex-request-id` for support. Revoked keys lose existing session and host streams. Suspension denies
all account access; resumption does not revive revoked keys.

Configured limits cover accounts, keys, hosts, claims, retained sessions, active turns,
streams, requests, body sizes and storage admission. Completed claims and deletion tombstones
remain retained in MVP; size metadata limits for the preview duration. No implicit historical
key reuse or metadata pruning occurs.

## Account API

The trusted website verifies Google OIDC and calls `POST /v1/accounts` with `AEX_SITE_TOKEN`.
It receives a seven-day dashboard credential, kept in a Secure HttpOnly SameSite cookie.
`GET /v1/account` and `GET /v1/usage` accept account session credentials or workload API keys.
`GET/POST /v1/keys`, `PATCH/DELETE /v1/keys/{id}`, and `DELETE /v1/account/session`
require account session credentials. Workload keys cannot issue keys. Keys are shown once,
stored as verifiers, named, listed, renamed and revoked within their account.
The first release is free hosting with customer model keys; it does not collect payments.

## Browser and CLI login

Dashboard, SDK and CLI share these public account APIs. The website keeps its bearer
credential in an HttpOnly cookie and forwards browser requests to the same API;
CLI and SDK use bearer credentials directly. Product rules live in Aex.

`POST /v1/auth/grants` requires an account session and accepts `code_challenge` (S256,
base64url SHA-256) and `redirect_uri` (`http://127.0.0.1:PORT/callback`). It returns
`{code, expires}` after the browser user authorizes the CLI. One grant per browser
session is retained, expires after 60 seconds, and is invalidated by browser logout.

`POST /v1/auth/exchange` accepts `{code, code_verifier, redirect_uri}` without a bearer
token. A matching, unexpired code is consumed atomically and returns a distinct
`{token, expires}` account session. Codes are stored hashed; intercepted codes cannot
be exchanged without the initiating client's PKCE verifier. Account sessions expire
after seven days, and at most eight remain active per account across web and CLI.

The CLI opens `/cli` on the website, completes Google sign-in if needed and receives
the code on its loopback listener. State binds the callback to the initiating process.
This follows the external-browser and loopback pattern in RFC 8252 and S256 PKCE in
RFC 7636. The website handles identity-provider integration, not separate product APIs.

Image and PDF uploads use the [attachment API](attachments.md). `models(provider?)` exposes only the configured hosted model choices, with models.dev metadata when known.
