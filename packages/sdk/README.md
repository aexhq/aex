# @aexhq/sdk

The TypeScript SDK and bundled CLI for durable aex agent sessions.

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
Runnable projects are collected in
[`aexhq/examples`](https://github.com/aexhq/examples).

The package also includes the CLI:

```bash
npx aex --help
```

That binary is a copy of the bundle published as `@aexhq/cli`, and the two are
the same bytes at the same version — CI compares them byte for byte on every
commit and publishes both whenever either changes. Both packages install a
command called `aex`, so install exactly one of them: with both installed, the
winner is package-manager ordering.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
