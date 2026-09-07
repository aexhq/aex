# Hosted API

Use `@aexhq/brain` 0.18.0 against Aex with an issued account key. Brain source is pinned by
full revision in Cargo. Customer keys never authorize direct access to private Brain.

| Methods | Path | Authorization |
| --- | --- | --- |
| POST, GET | `/v1/sessions` | Active key; lists contain owned sessions only |
| GET, DELETE | `/v1/sessions/{id}` | Account ownership |
| GET | `/v1/sessions/{id}/transcript` | Account ownership |
| GET | `/v1/sessions/{id}/events?after=N` | Ownership; JSON or SSE by Accept header |
| POST | `/v1/sessions/{id}/messages`, `/cancel`, `/end` | Ownership and operation key |
| POST | `/v1/agentloops` | Active key and approved Component bytes |
| GET | `/v1/agentloops/{id}` | Active key and approved content address |
| POST | `/v1/hosts` | Active account key |
| GET | `/v1/hosts/{id}/commands` | Scoped token and active issuing key/account |
| POST | `/v1/hosts/{id}/results`, `/events` | Scoped token and owned target session |
| GET | `/health/live`, `/health/ready` | No customer data |

Other methods/routes fail closed. Arbitrary Tool uploads, remote Environment URLs, execution
callbacks, provider endpoint overrides and hosted secret/filesystem/network grants are denied.
Custom Tools run in the customer's live process. Committed event cursors and shapes are
preserved. Keepalive comments have no event identity and are inserted only between frames.
Server-error text is redacted while retaining the Brain error code. Bodies and credentials
are never logged.

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
