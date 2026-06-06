---
title: Runs
description: The durable unit aex submits, observes, and archives.
icon: Play
---

A run is an immutable request to execute an agent task. The call site supplies the model, prompt, optional system message, composition primitives, output capture policy, and one inline `secrets` bundle. aex snapshots the non-secret inputs, vaults the secrets for the run lifetime, dispatches to the selected runtime, and records status, typed events, and outputs.

```ts
const runId = await aex.submitRun({
  provider: "anthropic",
  model: "claude-haiku-4-5",
  prompt: "Write the report and save it as a file.",
  secrets: { anthropic: { apiKey: process.env.ANTHROPIC_API_KEY! } }
});

for await (const event of aex.stream(runId)) {
  console.log(event.type);
}

await aex.wait(runId);
await aex.download(runId, { to: "./run.zip" });
```

The same durable record backs SDK and CLI reads. `aex.get(runId)` reads the run record, `aex.events(runId)` reads the captured event snapshot, `aex.stream(runId)` polls the `RunEvent` shape, `aex.streamEnvelopes(runId)` tails the coordinator WebSocket envelope, and `aex.download(runId)` assembles the whole run archive client-side.

Use `idempotencyKey` when retrying a submit from your own workflow. aex hashes the normalized non-secret submission, so a retry with the same key and same request body returns the existing run while a mismatched body fails with an idempotency conflict.
