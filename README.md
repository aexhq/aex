<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./assets/aex-mark-white.webp" />
    <source media="(prefers-color-scheme: light)" srcset="./assets/aex-mark-black.webp" />
    <img src="./assets/aex-mark-black.webp" alt="" width="44" align="absmiddle" />
  </picture>
  Aex
</h1>

<p align="center"><strong>Minimal and extensible backend for your AI workloads.</strong></p>
<p align="center">
  High-performance, reliable, and simple infrastructure for running AI workloads.<br />
  Start a session with your models and tools, give it work, and get back structured data.
</p>
<p align="center">
  <a href="https://aex.dev">Website</a> ·
  <a href="docs/quickstart.md">Quickstart</a> ·
  <a href="https://aex.dev/dashboard">Dashboard</a>
</p>

> This repo is under early and heavy development

Aex is the hosted composition around [Brain](https://github.com/aexhq/brain), a topology-neutral
session engine. Brain keeps one ordered journal on disk, derives the model transcript from it, calls
model providers, and routes Agentloops and Tools to their declared Environments. Aex adds identity,
billing, hosted policy, and deployment.

## Quickstart

```sh
npm install @aexhq/sdk @aexhq/env-aws-microvm @aexhq/agentloop-pi @aexhq/tools
```

```ts
import { Aex, brainEnv } from "@aexhq/sdk";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/agentloop-pi";
import { bash, read, write } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const workspace = awsMicroVm({ name: "sandbox", url: process.env.ENVIRONMENT_URL, token: process.env.ENVIRONMENT_TOKEN, region: "eu-west-2" });
const session = await aex.sessions.create({
  model: {
    provider: "vercel-ai-gateway",
    name: "openai/gpt-5-mini",
    apiKey: process.env.VERCEL_AI_GATEWAY_API_KEY!,
  },
  agentloop: pi({ env: brainEnv({ name: "brain" }) }),
  system: "Work carefully and verify changes.",
  tools: [bash({ env: workspace }), read({ env: workspace }), write({ env: workspace })],
});

await session.send("Inspect the workspace.");
for await (const event of session.events()) console.log(event);
```

The [TypeScript quickstart](docs/quickstart.md) covers typed Brain, Tool, and Environment
composition, operation keys, and durable event cursors.

## Architecture

```text
Your app → Aex SDK → Aex control → Brain → model provider
                                      ├── brain-sessions → session semantics
                                      ├── brain-env → Component worker pool
                                      ├── hostEnv → Tool in your app
                                      └── remote Environment
```

- **Agentloops** own model-call and context policy and run where their binding places them.
- **Native execution** runs WebAssembly Components in Brain with explicit filesystem, network, and
  secret capabilities.
- **Application Tools** use hostEnv, through the same session Environment abstraction.
- **Environment adapters** implement setup, execution, cancellation, detach, and teardown; callers choose their lifetime.

Brain's standalone executable stores the canonical journal on local disk. External journal stores
and durable hosted placement are roadmap items; applications can persist the ordered event feed.

## Tool placement

Definitions and bindings are fixed at session creation. A Tool may run in the application, in
Brain's Wasmtime Environment, or in a remote Environment such as a MicroVM, browser, sandbox,
Lambda, or HTTP service. Declare a factory in several named Environments to authorize each pair.
An Agentloop can select a fixed placement or present authorized choices to the model.

## Packages

| Package | Purpose |
| --- | --- |
| [`@aexhq/sdk`](packages/sdk) | Hosted authentication and the neutral Brain session API |
| [`@aexhq/contracts`](packages/contracts) | Generated control-plane types |
| [`@aexhq/cli`](packages/cli) | Command-line workflows |

[`brain`](https://github.com/aexhq/brain) owns the neutral session runtime and protocols.
[`extensions`](https://github.com/aexhq/extensions) contains official Brain, Tool, and Environment
extensions. This repository owns the public SDK, control plane, and hosted Aex composition.

## Development

Requires Rust 1.97, Node.js 22 or later, Python 3, and `cargo-typify`.

```sh
tools/gen.sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm ci
npm test
```

Change schemas before generated files, then run `tools/gen.sh`. Use `docker compose up --build` for
the trusted local composition.

[Hosted runtime](docs/hosted-runtime.md) ·
[Session API](https://github.com/aexhq/brain/blob/main/crates/brain-http/generated/contract/session/v1/openapi.yaml) ·
[Control API](contracts/control/v1/openapi.yaml)

Licensed under [Apache 2.0](LICENSE).

Brain is a standalone runtime consumed through its public contracts. Its default turn-end
suspension releases execution while transcripts and recorded Events remain readable. Aex owns
account, admission, billing, and future platform durability; callers choose Environment lifetime while providers enforce physical
resource ceilings. Tool/environment failures and interrupted turns remain explicit observations, with
no automatic effect replay in Brain.
