# @aexhq/sdk

The TypeScript SDK and bundled CLI for durable aex agent sessions.

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

const result = await session.messages.send("Summarize this repository.").finished();
console.log(result.status, result.text, result.costUsd);

const resumed = await aex.sessions.open(session.id);
console.log(resumed.record.acceptsMessages, resumed.record.lastRun);
```

The public surface is organized by ownership:

- `aex.sessions`: create, reopen, get, list, and delete sessions.
- `session.messages`, `session.events`, `session.files`: stable session-owned resources.
- `aex.workspace.files`, `.skills`, `.tools`, `.instructions`, `.secrets`: reusable workspace resources.
- `aex.start()`: the one-shot create, send, and finish convenience.

Session submissions accept immutable pinned refs under `assets`, with builtin
capabilities configured separately through `builtinTools`. A successful
`RUN_FINISHED` is the consistency boundary: checkpointed files, usage, cost,
messages, and session state are committed before `finished()` resolves.

See the [Quickstart](docs/quickstart.md), [Events](docs/events.md),
[Files](docs/files.md), and [Composition](docs/concepts/composition.md) guides.

The package also includes the CLI:

```bash
npx aex --help
```
