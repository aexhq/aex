<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./assets/aex-mark-white.webp" />
    <source media="(prefers-color-scheme: light)" srcset="./assets/aex-mark-black.webp" />
    <img src="./assets/aex-mark-black.webp" alt="" width="44" align="absmiddle" />
  </picture>
  Aex
</h1>

<p align="center">
  Hosted <a href="https://github.com/aexhq/brain">Brain</a> and extensions, with accounts and metered services.
</p>

<p align="center">
  Bring your models and tools. Run sessions, listen to events, and keep what you need.
</p>

<p align="center">
  <a href="https://aex.dev">Website</a> · <a href="https://aex.dev/docs">Docs</a> · <a href="https://aex.dev/dashboard">Dashboard</a>
</p>

> [!NOTE]
> Aex is in early preview. APIs and limits may change.
> Existing preview accounts keep free hosting. Prepaid services require explicit price acceptance;
> bring your own model-provider keys.

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
npm install @aexhq/sdk@0.78.0 @aexhq/agentloop-pi@7.0.0
```

```ts
import { Aex, brainEnv } from "@aexhq/sdk";
import { pi } from "@aexhq/agentloop-pi";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
try {
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
} finally {
  await aex.close();
}
```

Your script runs in your application; the session runs in hosted Brain.
Consume its events and store your own results. Brain retains history until deletion or expiry.

## Brain, hosted

Brain owns sessions, execution and events. Aex hosts Brain and extensions and adds accounts,
API keys, usage, prepaid credits and hosted access.
The Aex SDK extends and re-exports Brain, so its SDK and extension contracts remain directly usable.

The dashboard, SDK and CLI use the same Aex HTTP API.
Manage keys, inspect credit balances and usage, set a spend limit, top up through Stripe Checkout,
and refund unused credits. See [billing and recovery](docs/billing.md).

`session.submit()` returns the committed `turn_started` sequence without keeping a website request
open. Read events and the transcript later. Closing the client does not cancel that hosted turn;
host Tools still require their own process to remain connected.

## Models and tools

Use your own model keys and Brain-compatible extensions.
Hosted Wasm Agentloops and Tools run in Brain; `hostEnv` Tools run in your application.
SDK 0.78 uses Brain 0.28 and official loops 7.0. Custom Wasm Components must target
the matching WIT contract. Prepare application Tool dependencies before registration;
extensions no longer declare `needs`. Hosted `brainEnv` configuration must remain empty.

The preview does not enable arbitrary remote HTTP Environments or general shell execution.
For Python, data files and retrieval, select a published [managed Modal Environment](docs/environments.md).
Its finite sandbox shares a workspace across Tools, reserves prepaid credits before allocation,
and contains no platform or model credentials. Official loops can validate structured output and
perform bounded corrections inside the hosted turn, with tools disabled during correction.
See the [supported API](docs/api.md) and [SDK guide](packages/sdk/README.md) for the current boundaries.

## Packages

| Package | Purpose |
| --- | --- |
| [@aexhq/sdk](packages/sdk) | Brain's SDK with Aex account methods and hosted defaults |
| [@aexhq/cli](packages/cli) | Browser login, key CRUD, account, billing status, usage and docs |

For source builds and self-hosting, see [Development](docs/development.md)
and [Operations](docs/operations.md). Product plans live in the [roadmap](ROADMAP.md).

Licensed under [MIT](LICENSE).
