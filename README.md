<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./assets/aex-mark-white.webp" />
    <source media="(prefers-color-scheme: light)" srcset="./assets/aex-mark-black.webp" />
    <img src="./assets/aex-mark-black.webp" alt="" width="44" align="absmiddle" />
  </picture>
  Aex
</h1>

<p align="center">
  A thin, official hosted server for <a href="https://github.com/aexhq/brain">Brain</a>.
</p>

<p align="center">
  Bring your models and tools. Run sessions, listen to events, and keep what you need.
</p>

<p align="center">
  <a href="https://aex.dev">Website</a> · <a href="https://aex.dev/docs">Docs</a> · <a href="https://aex.dev/dashboard">Dashboard</a>
</p>

> [!NOTE]
> Aex is in early preview. APIs and limits may change.
> Hosting is currently free; bring your own model-provider keys.

## Get started

Sign in and create an API key in the [dashboard](https://aex.dev/dashboard),
or use the CLI with Node.js 22 or newer:

```sh
npm install -g @aexhq/cli
aex login
aex keys create "My application"
```

Login opens your browser. Save the new API key as `AEX_API_KEY`
and your OpenAI key as `OPENAI_API_KEY`, then install the SDK:

```sh
npm install @aexhq/sdk@0.72.0 @aexhq/agentloop-pi@5.0.0
```

```ts
import { Aex, brainEnv } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const session = await aex.sessions.create({
  agentloop: pi({ env: brainEnv({ name: "brain" }) }),
  model: {
    provider: "openai",
    name: "gpt-4.1-mini",
    apiKey: process.env.OPENAI_API_KEY!,
  },
});

try {
  await session.send("Explain what an agent session is in one sentence.");
  for await (const event of session.events()) console.log(event);
} finally {
  await session.end();
}
```

Your script runs in your application; the session runs in hosted Brain.
Consume its events and store your own results. Brain retains history until deletion or expiry.

## Brain, hosted

Brain owns sessions, execution and events. Aex adds accounts, API keys, usage and hosted access.
The Aex SDK extends and re-exports Brain, so its SDK and extension contracts remain directly usable.

The dashboard, SDK and CLI use the same Aex HTTP API.
Manage keys, view your account and usage, or check billing status from whichever client you prefer.

## Models and tools

Use your own model keys and Brain-compatible extensions.
Hosted Wasm Agentloops and Tools run in Brain; `hostEnv` Tools run in your application.
SDK 0.71 uses Brain 0.20 and official extensions 5.x. Custom Wasm Components must target
the matching WIT contract. Prepare application Tool dependencies before registration;
extensions no longer declare `needs`. Hosted `brainEnv` configuration must remain empty.

The preview does not enable arbitrary remote HTTP Environments or general shell execution.
See the [supported API](docs/api.md) and [SDK guide](packages/sdk/README.md) for the current boundaries.

## Packages

| Package | Purpose |
| --- | --- |
| [@aexhq/sdk](packages/sdk) | Brain's SDK with Aex account methods and hosted defaults |
| [@aexhq/cli](packages/cli) | Browser login, key CRUD, account, billing status, usage and docs |

For source builds and self-hosting, see [Development](docs/development.md)
and [Operations](docs/operations.md). Product plans live in the [roadmap](ROADMAP.md).

Licensed under [MIT](LICENSE).
