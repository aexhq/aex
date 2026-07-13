# aex

**Agent Executor.** aex is a platform for durable autonomous agent sessions,
available through a TypeScript SDK and CLI.

## Install

```bash
npm i @aexhq/sdk
```

```ts
import { Aex, Models } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);
const session = await aex.sessions.create({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
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

The package also includes the CLI:

```bash
npx aex start \
  --api-key "$AEX_API_KEY" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write a short report." \
  --follow
```

## Docs

- [Quickstart](packages/sdk/docs/quickstart.md)
- [Composition](packages/sdk/docs/concepts/composition.md)
- [Events](packages/sdk/docs/events.md)
- [Files](packages/sdk/docs/files.md)
- [Provider/runtime capabilities](packages/sdk/docs/provider-runtime-capabilities.md)

## Examples

Runnable TypeScript, CLI, and skill projects live in the canonical
[`aexhq/examples`](https://github.com/aexhq/examples) repository.

## Contribute

- [Contributor flow](CONTRIBUTING.md)
- [Security disclosures](SECURITY.md)
- [Apache License 2.0](LICENSE)
