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
