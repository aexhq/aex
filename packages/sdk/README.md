# Aex SDK

Install `@aexhq/sdk`. Create an API key at https://aex.dev/dashboard.

```ts
import { Aex, agentloop, brainEnv, component, hostEnv, tool } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
console.log(await aex.account.get());
```

`Aex` extends the pinned Brain client. Its sessions, registration, Events, Components and
extension builders are Brain's implementations. Existing Brain extension packages work with
the same contracts. See https://aex.dev/docs for a complete session example.

Place uploaded Wasm Agentloops and Tools in `brainEnv` for hosted execution. `hostEnv`
executes application functions in your process. Native hosted Components receive no server
secrets, filesystem or network grants. Customer-selected HTTP Environments are not enabled
on the initial deployment. Supply your model key per session.

`account.get()` and `account.usage()` accept a workload API key. `keys` management requires
an authenticated account session and is normally performed in the dashboard. API keys do
not grant authority to create more credentials. New key secrets are returned once.

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
