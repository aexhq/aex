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

Aex is the hosted composition around [Brain](https://github.com/aexhq/brain), an ephemeral,
topology-neutral execution kernel. Brain keeps current context in memory, journals execution to
disk, runs one universal Agentloop Component format, calls a remote model gateway, and routes Tool
operations to remote Environments. Aex adds identity, shared resources, placement, and deployment.

## Quickstart

```sh
npm install @aexhq/sdk @aexhq/env-aws-microvm @aexhq/loop-pi @aexhq/tools
```

```ts
import { Aex } from "@aexhq/sdk";
import { awsMicroVm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/loop-pi";
import { bash, read, write } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const workspace = awsMicroVm({ region: "eu-west-2" });
const session = await aex.createSession({
  model: {
    provider: "vercel-ai-gateway",
    name: "openai/gpt-5-mini",
    apiKey: process.env.VERCEL_AI_GATEWAY_API_KEY!,
  },
  agentLoop: pi(),
  system: "Work carefully and verify changes.",
  tools: [bash().runIn(workspace), read().runIn(workspace), write().runIn(workspace)],
});

await session.send("Inspect the workspace.");
for await (const event of session.events()) console.log(event);
```

The [TypeScript quickstart](docs/quickstart.md) covers typed Agentloop, Tool, and Environment
composition, operation keys, and durable event cursors.

## Architecture

```text
Your app → Aex SDK → Aex control → Brain Server → remote Environment
                                      │                 │
                              Agentloop Component   Tool execution
                                      │
                               remote model gateway
```

- **Agentloop Components** use one capability-pure Wasm contract.
- **Models** are trusted remote bindings shared by Brain Server.
- **Tool definitions** are stable model presentation; implementations run in Environments.
- **Environment adapters** own setup, attachment, execution, cancellation, and teardown.

Brain's standalone executable stores its journal on disk and current context in memory. Hosted
durability, Environment identity, session placement, and queue bridges are downstream Aex concerns.

## Tool placement

Definitions and bindings are sealed at session creation. Tool implementations never run inside
Brain: an Environment may be a MicroVM, browser, sandbox, or user process, and several sessions may
bind to the same logical Environment across Brain Server tasks.

## Packages

| Package | Purpose |
| --- | --- |
| [`@aexhq/sdk`](packages/sdk) | Hosted authentication and the neutral Brain session API |
| [`@aexhq/contracts`](packages/contracts) | Generated control-plane types |
| [`@aexhq/cli`](packages/cli) | Command-line workflows |

[`brain`](https://github.com/aexhq/brain) owns the neutral session kernel and protocols.
[`extensions`](https://github.com/aexhq/extensions) contains official Agentloop, Tool, and Environment
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
[Session API](https://github.com/aexhq/brain/blob/main/contracts/session/v1/openapi.yaml) ·
[Control API](contracts/control/v1/openapi.yaml)

Licensed under [Apache 2.0](LICENSE).
