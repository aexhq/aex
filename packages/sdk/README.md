# Aex SDK

Install `@aexhq/sdk`. Create an API key at https://aex.dev/dashboard.

```ts
import { Aex, agentloop, brainEnv, component, hostEnv, tool } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
console.log(await aex.account.get());
```

`Aex` extends the pinned Brain client. Its sessions, registration, Events, Components and
extension builders are Brain's implementations. SDK 0.73 uses Brain SDK 0.22 and Pi/Codex/Tools
5.1. Brain 0.22 retains the existing Wasm interface. Components and session configurations are
not migrated automatically. See https://aex.dev/docs
for a complete session example.

Host Tools return ordinary successful output or a Brain `Outcome` directly. Structured errors retain
code, message, retryable and details. The top-level statuses `ok`, `error`, `timeout`, `cancelled`
and `unknown` declare outcomes; malformed envelopes fail validation, and only successful values
pass through the output schema. Tool deadlines produce `timeout`, explicit cancellation produces
`cancelled`, and missing reliable results after dispatch produce `unknown`. All are failed Tool
results. Timeout and cancellation do not promise rollback. See
[the full return contract](https://aex.dev/brain/docs/guides/write-a-tool#return-values-and-outcomes).

`@aexhq/tools-mcp` runs selected MCP Tools in your application host, preserving structured failures
and original JSON Schema constraints. The official Docker and browser HTTP Environments target
standalone Brain and require operator deployment. See [official extensions](https://github.com/aexhq/extensions).

Place uploaded Wasm Agentloops and Tools in `brainEnv` for hosted execution. `hostEnv`
executes application functions in your process. Native hosted Components receive no server
secrets, filesystem or network grants. Customer-selected HTTP Environments are not enabled
on the initial deployment. Hosted `brainEnv` configuration must be empty; resource access
is configured by each Environment, not declared through extension `needs`. Prepare dependencies
for `hostEnv` Tools in your application before registering them. Supply your model key per session.

`account.get()` and `account.usage()` accept a workload API key. `keys` management requires
an authenticated account session and is normally performed in the dashboard. API keys do
not grant authority to create more credentials. New key secrets are returned once.

## Structured output

Pass `output: { type: z.object({ name: z.string() }), maxRetries: 2 }` in the second
argument to `session.send`. The SDK prompts for JSON, validates with Zod locally,
and returns the inferred parsed value. Two additional correction turns are allowed
by default; set zero to disable retries. Ordinary sends keep returning session state.

See the [executable example](https://github.com/aexhq/aex/blob/main/examples/structured-output.mjs)
and [full contract](https://aex.dev/brain/docs/guides/structured-output).
Exhaustion throws `StructuredOutputError` with `attempts`, `lastOutput`, and `issues`.
Retries run in your client and are ordinary turns with the session's existing tools.
Use exclusive ownership of sends during the operation. An optional top-level
`signal` cancels the active turn and stops corrections. Raw attempts remain visible
in history and streams; validation does not change the Agentloop or provider format.

## Account management

Use an account session obtained through browser login for account management. Workload
API keys remain scoped to runtime operations and read-only account/usage access.

```ts
const account = new Aex({ accountToken: process.env.AEX_ACCOUNT_TOKEN! });
await account.keys.list();
const issued = await account.keys.create({ name: "My app" });
await account.keys.update(issued.key.id, { name: "Renamed app" });
await account.keys.delete(issued.key.id);
await account.account.get(); // includes billing status and limits
await account.account.usage();
await account.account.logout();
```

`account.account.authorizeLogin({code_challenge, redirect_uri})` and
`Aex.exchangeLogin({code, code_verifier, redirect_uri})` expose the shared HTTP login
contract for clients. The `@aexhq/cli` package handles opening the browser, PKCE,
loopback callback and local credential storage.
