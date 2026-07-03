---
title: Runs
description: The durable unit aex submits, observes, and archives.
icon: Play
---

A run is a durable, resumable **session**: the model, system message, composition
primitives, output policy, and per-provider keys you open it with, plus every
turn you send to it. aex snapshots the non-secret inputs, holds secrets for the
session lifecycle, dispatches each turn through the managed runtime, and records
status, typed events, and outputs. Sessions are the low-level API; `run()` is the
one-shot convenience wrapper over them.

```ts
import { Aex, Models } from "@aexhq/sdk";

const aex = new Aex({ apiToken: process.env.AEX_API_TOKEN! });

const session = await aex.openSession({
  provider: "anthropic",
  model: Models.CLAUDE_HAIKU_4_5,
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

const turn = session.send("Write the report and save it as a file.");
for await (const event of turn) {
  console.log(event.type);
}
await turn.done();

await session.wait();
await session.download({ to: "./session.zip" });
```

The same durable record backs SDK and CLI reads. From the handle use `refresh`,
`unit`, `wait`, and `download` for lifecycle, plus the grouped read accessors —
`messages()`, `events()`, and `outputs()` (each with `list()`/`last()`/`first()`,
and `events().stream()` / `events().streamEnvelopes()` / `outputs().read(...)` for
streaming and byte-capped reads) — to inspect the session live or after it parks;
from the client, `aex.sessions.list()` / `aex.sessions.get(id)` read across the
workspace (CLI mirrors: `aex sessions` and `aex runs` list the workspace's
sessions/runs newest-first).

Use `idempotencyKey` when retrying `openSession` or `send` from your own
workflow. aex hashes the normalized non-secret submission, so a retry with the
same key and same body returns the existing session while a mismatched body fails
with an idempotency conflict.

aex selects product placement server-side. There is no region selector.
