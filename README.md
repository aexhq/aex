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
  <a href="https://aex.dev/dashboard">Dashboard</a> ·
</p>

> This repo is under early and heavy development

Aex is a session-oriented backend for AI applications, built on a minimal and extensible
kernel ([Brain](https://github.com/aexhq/brain)). The kernel owns mechanism — durable
sessions, journaled effects, recovery; your agent's behavior is extension policy. It keeps
the model loop and context durable, then starts isolated compute only when a tool needs it.
Bring your own model key, choose the tools a session can use, and receive text or validated
data.

## Quickstart

```sh
npm install @aexhq/sdk @aexhq/env-aws-microvm @aexhq/loop-pi @aexhq/tools
```

```ts
import { Aex } from "@aexhq/sdk";
import { awsMicrovm } from "@aexhq/env-aws-microvm";
import { pi } from "@aexhq/loop-pi";
import { bash, read, write } from "@aexhq/tools";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const workspace = awsMicrovm();
const session = await aex.sessions.create({
  model: {
    provider: "openai",
    name: "gpt-5.4",
    apiKey: process.env.OPENAI_API_KEY!,
  },
  loop: pi(),
  environments: { workspace },
  tools: [bash(), read(), write()],
});

const reply = await session.send("Plan my day.");
console.log(reply);
```

The [TypeScript quickstart](docs/quickstart.md) covers tools, structured output, files, storage,
and subagents.

## Architecture

```text
Your app → Aex SDK → Brain session kernel → bound environment extensions
                         ↑                         ↑
                 brain extension            tool extensions
```

- **Brain extensions** implement agent-loop policy.
- **Tool extensions** define capabilities available to the model.
- **Environment extensions** execute bound tools and own their runtime lifecycle.

Brain owns durable session mechanism, not a default loop or environment. The SDK auto-binds an
unbound tool only when exactly one declared environment is compatible.

Session journals live in the database. Files become durable only when copied to storage.

## Tool placement

Tools and bindings are fixed when a session is created. Use `tool.bind(environmentRef)` when more
than one environment is compatible. The `app()` environment runs callbacks in your application;
`aex tools build` prepares a computer tool with its own runtime and immutable dependencies.

## Packages

| Package | Purpose |
| --- | --- |
| [`@aexhq/sdk`](packages/sdk) | Sessions, tools, files, storage, and structured output |
| [`@aexhq/environment`](packages/environment) | Environment extension authoring contracts |
| [`@aexhq/contracts`](packages/contracts) | Generated control-plane types |
| [`@aexhq/cli`](packages/cli) | Command-line workflows |

[`brain`](https://github.com/aexhq/brain) owns the neutral session kernel and protocols.
[`extensions`](https://github.com/aexhq/extensions) contains official loop, tool, and environment
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
