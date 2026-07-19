---
title: Quickstart
---

# Quickstart

## Install

```bash
npm i @aexhq/sdk
```

Set an aex workspace key and the BYOK key for your model:

```bash
export AEX_API_KEY="<your-aex-api-key>"
export ANTHROPIC_API_KEY="<your-anthropic-api-key>"
```

The workspace key needs `sessions:read`, `sessions:write`, and `files:read` for
this workflow. Add `billing:read` when the application also reads cost and
billing-account resources.

## Run a session

```ts
import { Aex, Models, Sizes } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);
const session = await aex.sessions.create({
  model: Models.CLAUDE_HAIKU_4_5,
  system: "You are a concise engineering assistant.",
  runtime: Sizes.CPU_0_25_1GB,
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

const run = session.messages.send("Write a short report and save it as a file.");
for await (const event of run) {
  console.log(event.type, event.runId);
}

const result = await run.finished();
console.log(result.status, result.costUsd, result.text);
```

`finished()` resolves only after `RUN_FINISHED` or `RUN_ERROR`. A
`RUN_FINISHED` result is checkpoint-consistent: its session record, cost,
usage, messages, and files all reflect the same committed run. A `RUN_ERROR`
that failed before a checkpoint has `files: []` and no `checkpoint`.

Held outcomes remain explicit. `suspended` and `awaiting_approval` are not
reported as successful runs, and `result.ok` is true only for `succeeded`.

## Reopen and continue

```ts
const resumed = await aex.sessions.open(session.id);
if (resumed.record.acceptsMessages) {
  await resumed.messages.send("Validate the report and summarize the result.").finished();
}
```

`record.currentRun` describes active work and `record.lastRun` describes the
most recently completed or held run.

## Publish reusable inputs

Workspace resources are versioned and immutable when submitted. Publish local
drafts first, then pass the returned pinned refs under `assets`:

```ts
import { File } from "@aexhq/sdk";

const source = await aex.workspace.files.publish(
  await File.fromPath("./input.csv", { mountPath: "/workspace/input" })
);

const withInput = await aex.sessions.create({
  model: Models.CLAUDE_HAIKU_4_5,
  assets: { files: [source] },
  builtinTools: "default",
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

The same pattern applies to `aex.workspace.skills`, `.tools`, and
`.instructions`. Raw uploaded bytes are assets; workspace resources add typed,
versioned meaning to those bytes.

## Read checkpointed files

```ts
const completed = await withInput.messages.send("Create output/report.md").finished();
const snapshot = await withInput.files.list({
  checkpointId: completed.checkpoint?.checkpointId
});

console.log(snapshot.revision, snapshot.files);
const report = await withInput.files.findOne({ filename: "report.md" });
if (report) {
  console.log((await withInput.files.read(report)).text);
}
```

Session file IDs are meaningful only with their checkpoint. File objects carry
`checkpointId`, and ID selectors must include it.

## One-shot convenience

`aex.start()` is the one retained convenience for create, send, and finish:

```ts
const result = await aex.start({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Summarize this repository.",
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

console.log(result.sessionId, result.status, result.text);
```

The bundled CLI provides the same one-shot workflow:

```bash
npx aex start \
  --api-key "$AEX_API_KEY" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write a short report and save it as a file." \
  --follow
```

## Next

- [Composition](concepts/composition.md)
- [Events](events.md)
- [Files](files.md)
- [Webhooks](webhooks.md)
- [Provider/runtime capabilities](provider-runtime-capabilities.md)
