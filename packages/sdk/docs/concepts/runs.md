---
title: Runs
description: The durable unit aex submits, observes, and archives.
icon: Play
---

A run is an immutable request to execute an autonomous agent task. The caller
submits the model, prompt, optional system message, composition primitives,
output policy, and one inline secrets bundle. aex snapshots the non-secret
inputs, holds secrets for the run lifecycle, dispatches through the managed
runtime, and records status, typed events, and outputs.

```ts
import { AgentExecutor, Models } from "@aexhq/sdk";

const aex = new AgentExecutor({ apiToken: process.env.AEX_API_TOKEN! });

const runId = await aex.submit({
  provider: "anthropic",
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "Write the report and save it as a file.",
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});

for await (const event of aex.stream(runId)) {
  console.log(event.type);
}

await aex.wait(runId);
await aex.download(runId, { to: "./run.zip" });
```

The same durable record backs SDK and CLI reads. Use `getRun`/`get`, `events`,
`stream`, `streamEnvelopes`, `wait`, `outputs`, and `download` to inspect the
run live or after completion.

Use `idempotencyKey` when retrying a submit from your own workflow. aex hashes
the normalized non-secret submission, so a retry with the same key and same body
returns the existing run while a mismatched body fails with an idempotency
conflict.

Use the optional `region` submit field when you need a product placement target
such as `lhr`, `iad`, `sfo`, or `bom`. Region tokens select configured platform
backing for the run; they are not exact city guarantees. When omitted, aex
infers a configured region from request geography and falls back when no hint
matches.
