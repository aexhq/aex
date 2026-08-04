# aex

The public TypeScript contracts, SDK, and CLI for explicit aex v1 sessions.

[![npm version](https://img.shields.io/npm/v/@aexhq/sdk.svg)](https://www.npmjs.com/package/@aexhq/sdk)

> **Development status:** AEX is in active prelaunch development. Not all
> documented capabilities are deployed or verified, and the SDK, CLI, APIs,
> and hosted service are not guaranteed to work. Expect breaking changes and
> interruptions; do not rely on AEX for production workloads yet.

## Start

```bash
npm install @aexhq/sdk
```

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_WORKSPACE_API_KEY!);
const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5"
});
const { run } = await session.messages.send("Summarize this repository.");
const result = await run.result();

console.log(result.id, result.status);
```

Session creation, message admission, and run observation are separate
operations. Long-running lifecycle mutations and telemetry exports return
durable operation handles.

The CLI is a native binary published as a signed archive per target:

```bash
aex sessions create --request @session.json --api-key "$AEX_WORKSPACE_API_KEY"
```

## Public surfaces

| Surface | Purpose |
| --- | --- |
| [`api/schemas`](api/schemas) | Strict v1 wire schemas, identifiers, routes, errors, and scopes — the authoring source. |
| [`api/generated`](api/generated) | The generated bundle, OpenAPI documents, and per-schema JSON Schema. |
| [`crates/aex-wire`](crates/aex-wire) | The generated Rust wire contract: models, route table, errors, server and client shapes. |
| [`packages/sdk`](packages/sdk) | TypeScript client for bootstrap and regional resources. |
| [`tools/aex-cli`](tools/aex-cli) | Standalone resource-oriented `aex` command. |

Each public surface has its own immutable source identity in the release graph.
This workspace has not released `1.0.0`.

## Public v1 shape

- Sessions are created explicitly; messages admit durable runs.
- Workspace files, skills, tools, instructions, and MCP servers are
  overwrite-by-name registered resources.
- Session compute is requested by capacity; callers never select an execution
  implementation. Raw Hands networking is either `none` or
  `public_internet`.
- Persisted and live file reads are distinct and downloads use short-lived
  grants.
- Events, logs, spans, metrics, traces, telemetry gaps, and telemetry exports
  are first-class observational resources.
- Account, organization, workspace, API-key, billing, usage, and statement
  resources live on the bootstrap surface.

See the [SDK quickstart](apps/site/content/docs/), [registered
resources](apps/site/content/docs/), [files](apps/site/content/docs/),
[telemetry](apps/site/content/docs/), and [public
architecture](references/architecture.md).

## Repository boundary

This repository contains the public API artifacts and client packages. The
hosted control plane, scheduling, storage, billing implementation, dashboard,
runtime infrastructure, and deployment definitions live outside it.

## Contribute

- [Contributor flow](CONTRIBUTING.md)
- [Code of conduct](CODE_OF_CONDUCT.md)
- [Security disclosures](SECURITY.md)
- [Apache License 2.0](LICENSE) and [third-party attribution](NOTICE)
