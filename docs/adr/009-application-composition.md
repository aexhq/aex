# ADR-009: Compose browser and application tools through session creation

Accepted 2026-09-27. Supersedes ADR-004's prohibition on declared application endpoints.

Developers select an Environment, Tools and Agentloop, then call `aex.sessions.create()`.
Expose `clientBrowser({ name })` for the current connected tab and
`aex.environments.application({ name, endpoint, credential, timeoutMs })` for a deployed
backend. Keep one Environment contract and explicit tool placement. Generic `hostEnv`
and the automation `browser` extension retain their meanings.

Application creation admits the endpoint, bounded timeout and tool contracts under the
account's create claim. Credentials use Brain's sealed Environment storage. One mounted
SDK handler reuses declared tools, options and durable `ctx.finish` acknowledgments. Public
completion capabilities are invocation-scoped and relayed to the fixed private bridge.
Public HTTPS filtering and private service isolation remain required. Existing configured
HTTP sessions retain their v1 behavior.

Browser access is delegated by an authenticated application backend through
`aex.clients.grant({ origin, session, expiresAt })`. Brain prepares the composition without
opening a host connection; Aex registers the authorized host and issues scoped access.
Pending model credentials stay in bounded memory for at most five minutes, then transfer
to Brain during creation. Product records retain token hashes, a public-composition
fingerprint, exact component identities and ownership. Interruption before creation requires
fresh authorization; dispatched creation keeps the existing ambiguous-claim semantics.

The browser uses `new Aex({ clientAccess })` and ordinary `sessions.create()`. Its origin,
composition, host and one resulting session are fixed. Aex enforces server-side credential
resolution, expiry, revocation and suspension. Client grants cannot access account operations,
other sessions or arbitrary Component admission.

Closing a tab removes that execution context. Backend tool calls must fit the endpoint's
request lifetime; durable business jobs remain application-owned. Neither choice moves
application code into Brain or adds automatic retries or fallback placement.
