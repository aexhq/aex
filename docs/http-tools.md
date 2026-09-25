# Call tools in your application's API

Use HTTP tools when an agent needs short reads or proposals from your application. Aex runs
the agent; your existing API handles one authenticated request per tool call. A serverless
API needs no process connected while the model thinks. Your application still hosts its
business functions.

Install `@aexhq/env-http` with the Aex SDK and define tools using Brain's `tool()` function.
Mount `createToolHandler({ tools, authorize })` from `@aexhq/env-http/handler` at one POST
route. In Hono, return `handler(c.req.raw)`. The portable package's
[complete example](https://github.com/aexhq/extensions/blob/main/packages/env-http/examples/records.mjs)
demonstrates current membership checks, reads, proposals, atomic reviewed saves and report jobs.

An operator approves a binding for your account with an HTTPS endpoint, fixed tool contracts,
credential reference and call timeout. Call `await aex.environments.http()` to discover your
binding IDs, timeout limits and driver URL. Instantiate `http({ name: "app", url: catalog.driver_url,
binding: "your-binding-id" })`, and place each declared tool using `httpTool(placedTool, { env })`.
Pass those tools to the ordinary session create operation; lifecycle defaults to automatic.
See [lifecycle and diagnostics](environments.md#lifecycle-and-diagnostics)
for manual setup and optional model access to Environment methods.

Before submitting the first turn, save the mapping from the Aex session to the authenticated
application user and company. The route's authorizer verifies the bridge credential and checks
that mapping against current permissions. Tool arguments cannot choose another user or company.
Use the same business operations and authorization as ordinary application routes. For changes,
return a proposal; the user's reviewed save checks current data and commits the change and its
operation receipt together.

`session.submit(input, { idempotencyKey })` returns a committed turn sequence. Save it and close
the submitting client. A later request opens the session and calls `session.outcome(sequence)`
to distinguish pending, ended and failed work. Read the original terminal event on failure.
An idle session is not a save receipt or evidence that a background report has completed.

Configure structured answers on Pi or Codex using `output: { schema, maxCorrections }` so
formatting corrections happen inside the hosted turn with tools unavailable. Client-side
structured `send()` issues ordinary turns that can still invoke tools. See Brain's
[structured-output guide](https://aex.dev/brain/docs/guides/structured-output).

## Lifetime and failures

HTTP bindings have a per-call timeout and no sandbox lease or sandbox-time charge. A saved
session keeps its sealed binding on later turns. Aex checks account/key access and the current
account grant on every invocation. Credential rotation preserves the same endpoint and contract;
changing either requires a new binding ID and session. Removing access stops future dispatch.

Handlers return bounded JSON. Async Zod validation consumes the call deadline, and incompatible
advertised schemas fail before business code. `{status: "cancelled"}` can describe a report as
ordinary successful data. These handlers have no open background callbacks; use `start_report`
and `get_report` around your application's durable job store. Later inspection needs another
tool call or turn; Aex does not schedule it automatically.

Aex keeps callback grants and endpoint credentials in its trusted bridge. Application requests
contain only the authenticated invocation identity, sealed contract, input and deadline.
The default transport connects only to public HTTPS addresses, checks DNS at socket creation,
and refuses redirects. Deployments also restrict bridge egress independently of its code.

Lost or malformed replies stay unknown and are never retried automatically. Cancellation is
best effort and cannot roll back an external mutation. Keep stable application operation keys
and query the business receipt when a save acknowledgement is lost.

## Operator configuration

`http_environments` in the [generated configuration schema](generated/config.schema.json)
contains `url`, `public_url` and account-approved `bindings`. Each entry supplies `accounts`,
`credential_env`, and an immutable `specification` with `url`, `timeoutMs`, and Brain Tool
definitions (`name`, `description`, `input_schema`, optional `output_schema`). No credentials
belong in that document. The configured environment variable supplies the endpoint credential.

Run `environments/http.mjs` from the Aex image as its own unprivileged process. It listens on
loopback port 8084 and uses `AEX_ENVIRONMENT_TOKEN` to authorize against Aex's private operator
listener. Keep both listeners off public ingress. Its only mutable invocation state is in memory;
bindings and access remain in Aex's existing product store.
