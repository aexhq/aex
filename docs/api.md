# Hosted API

Aex SDK 1.0.0 composes Brain SDK 2.1.0. Shared session, executor, input and event contracts come from the pinned [Brain OpenAPI](../contracts/brain.openapi.json). Customer credentials authorize the Aex gateway; the private runtime has separate authority.

| Method | Path | Purpose |
| --- | --- | --- |
| POST, GET | `/sessions` | Create or list owned sessions |
| GET, DELETE | `/sessions/{id}` | Read or delete an owned session |
| POST, GET | `/sessions/{id}/events` | Submit input; read history or SSE |
| POST | `/sessions/{id}/resume`, `/sessions/{id}/close` | Explicit lifecycle operations |
| POST | `/executors` | Register application callbacks |
| GET, POST | `/executors/{id}/…` | Brain executor commands, results and closure |
| GET | `/tools`, `/v1/models` | Account-authorized catalogs |
| GET | `/composition` | Read scoped browser composition and bound session |
| POST, DELETE | `/v1/clients`, `/v1/clients/{id}` | Issue or revoke scoped browser access |
| GET | `/v1/account`, `/v1/usage`, `/v1/usage/{session}` | Account and durable usage evidence |
| POST | `/v1/sessions/{id}/attachments`, `/attachment-grants` | Upload or authorize an upload |
| GET, DELETE | `/v1/sessions/{id}/attachments/{attachment}` | Read metadata or revoke an attachment |
| GET, HEAD | `/v1/attachments/{id}/content?token=…` | Scoped file read capability |
| GET | `/health/live`, `/health/ready` | Product health |

Product account, key, billing, attachment and login shapes are generated from Rust in [generated](generated/). Unsupported operations fail explicitly. Old versioned Brain session routes, Wasm uploads, host registrations and Environment preparation are retired.

Inputs use `{type:"user.message", text, media?, key}`. Event IDs correlate accepted input and terminal execution; cursors are opaque strings. Pass them back unchanged. SSE comments have no event identity. SDK `submit()` returns acceptance; `send()` waits for that input's terminal result and returns a turn with `output`, `events`, `session` and `input_id`.

Use an idempotency key for creation and resource uploads. A message's `key` is scoped to its session. Reusing a key with different content fails. A committed acceptance can be read again without repeating work; an uncertain outcome stays unresolved. Do not generate a new key to retry a potentially executed action.

Provider endpoints, executable commands, tenant scope and service credentials belong to the deployment. Customers select admitted harnesses/models and authorized managed tools. Scope supplied in public headers cannot override account ownership. Errors use Brain's `{error:{code,message,details?}}` envelope.
