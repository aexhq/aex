---
title: Quickstart
---

# Quickstart

Install the SDK:

```bash
npm install @aexhq/sdk
```

Create an explicit session, admit a message, then wait on its durable run:

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_WORKSPACE_API_KEY!);
const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5"
});
const { run } = await session.messages.send(
  "Summarize this repository.",
  { idempotencyKey: "summarize-repository" }
);
const result = await run.result({
  pollIntervalMs: 1_000,
  timeoutMs: 300_000
});

console.log(result.id, result.status, result.outputMessageIds);
```

`run.result()` polls the canonical run resource. A timeout detaches this
client-side wait; it does not cancel the run.

The command-line interface is a separate install:

```bash
bun add --global @aexhq/cli
aex sessions create --request @session.json --api-key "$AEX_WORKSPACE_API_KEY"
```

Next: [registered resources](resources.md), [files](files.md), and
[telemetry](telemetry.md).
