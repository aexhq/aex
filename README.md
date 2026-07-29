# aex

**Agent Executor.** An open-source agent execution engine, and a hosted platform
that runs it for you.

An aex session is a durable, resumable thread that a model drives with real
tools — a shell, a filesystem, editors, web fetch and search, background
commands, code execution, git, and subagents — inside an isolated runtime, with
every event, file, and cost accounted for. It is the same problem AWS Bedrock
AgentCore and Anthropic's managed agents solve, with the engine published under
Apache 2.0 instead of held behind a service boundary.

[![npm version](https://img.shields.io/npm/v/@aexhq/sdk.svg)](https://www.npmjs.com/package/@aexhq/sdk)

## Start

```bash
npm i @aexhq/sdk
```

Model access is managed: name a model by its `creator/model` gateway slug and
the platform's key routes it. You supply no provider API key.

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);
const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5"
});

const result = await session.messages.send(
  "Write a short report and save it as a file."
).finished();

console.log(result.status, result.text, result.checkpoint);

const resumed = await aex.sessions.open(session.id);
await resumed.messages.send("Validate the report.").finished();
```

Reusable inputs are published through `aex.workspace.files`, `.skills`,
`.tools`, or `.instructions`, then submitted as version-pinned refs under
`assets`. Session outputs are read through checkpoint-aware `session.files`.

The same thing from a terminal:

```bash
npx aex start \
  --api-key "$AEX_API_KEY" \
  --model anthropic/claude-haiku-4-5 \
  --prompt "Write a short report." \
  --follow
```

## Open core, stated plainly

aex is open core. This repository is the open half, under
[Apache License 2.0](LICENSE):

- the public wire contracts, the TypeScript SDK, and the CLI — published today;
- the agent execution engine — the session loop, the builtin tool
  implementations, the runtime adapters, and the runtime-side protocol. These
  are being extracted from the private history package by package, with their
  commit lineage preserved; the [packages table](#packages) is the set that has
  landed so far.

The hosted half is a separate private repository: accounts, authentication,
billing and pricing, scheduling, the storage and database schema, the dashboard,
and the infrastructure definitions that run them.

**Running the hosted plane yourself is out of scope for this repository.** It
ships no control plane, no infrastructure definitions, and no deployment path,
and nothing in this documentation should be read as offering one. What Apache
2.0 gives you is the engine, its contracts, and the right to build on both.

If you are reading the engine source and hit a hostname that does not resolve —
`egress.internal`, `journal.internal`, and four siblings — see
[`references/internal-protocol.md`](references/internal-protocol.md). Those are
virtual hosts terminated by the managed boundary, not missing services.

## Packages

| Package | What it is |
| --- | --- |
| [`@aexhq/sdk`](packages/sdk) | The TypeScript SDK. Bundles the CLI as the `aex` binary. |
| [`@aexhq/cli`](packages/cli) | The CLI on its own, for callers that do not want the SDK. |
| [`@aexhq/contracts`](packages/contracts) | Wire schemas, the event envelope, id formats, and the generated OpenAPI document. |

Each is an independently versioned npm package with its own checks and its own
canary. Promotion of a canary to the stable dist-tag is always a manual gate.

The engine packages are being extracted into this repository from the private
history, package by package. Until that lands, this table is the whole published
set.

## Docs

- [Quickstart](packages/sdk/docs/quickstart.md)
- [Composition](packages/sdk/docs/concepts/composition.md)
- [Events](packages/sdk/docs/events.md)
- [Files](packages/sdk/docs/files.md)
- [Model access](packages/sdk/docs/provider-runtime-capabilities.md)
- [Architecture](references/architecture.md) — how the pieces fit, and where the
  boundary between this repository and the hosted plane falls.
- [Glossary](references/glossary.md) — `brain`, `hands`, `plane`, and the rest of
  the vocabulary the source uses without explaining.

The rendered documentation site is [aex.dev/docs](https://aex.dev/docs).

## Examples

Runnable TypeScript, CLI, and skill projects live in the canonical
[`aexhq/examples`](https://github.com/aexhq/examples) repository.

## Contribute

- [Contributor flow](CONTRIBUTING.md) — including the deliberate asymmetry
  between contributor pull requests and maintainer pushes.
- [Code of conduct](CODE_OF_CONDUCT.md)
- [Security disclosures](SECURITY.md)
- [Apache License 2.0](LICENSE) and [third-party attribution](NOTICE)
