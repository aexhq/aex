---
title: Quickstart
---

# Quickstart

## 1. Install

```bash
bun add @aexhq/sdk
```

This installs the TypeScript SDK exports and the bundled `aex` CLI.

## 2. Set credentials

In the dashboard, create a quickstart SDK token with `runs:read`, `runs:write`,
and `outputs:read`. The examples also need your BYOK provider key for the model
you choose. For the Claude examples below:

```bash
export AEX_API_TOKEN="<your-aex-token>"
export ANTHROPIC_API_KEY="<your-anthropic-api-key>"
```

## 3. Open a session

```ts
import { Aex, Models, Sizes } from "@aexhq/sdk";

const aex = new Aex({ apiToken: process.env.AEX_API_TOKEN! });

const session = await aex.openSession({
  model: Models.CLAUDE_HAIKU_4_5,
  system: "You are a concise engineering assistant.",
  runtime: Sizes.SHARED_0_25X_1GB,
  overrides: { idleTtl: "3m" },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

const first = await session.send("Write a short report and save it as a file.").done();
console.log(first.status, first.text);
```

The session parks as `idle` between turns and automatically moves to
`suspended` after the idle window. Keep the session id and resume later:

```ts
const resumed = await aex.openSession(session.id);
await resumed.send("Now run the validation command and summarize the result.").done();
```

## 4. One-shot convenience

`run()` opens a session, sends `message` as one turn, and returns the collected
result. Its `runId` is the session id.

```ts
const result = await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! },
  message: "Write a short report and save it as a file."
});

console.log(result.runId, result.status, result.text);
```

## 5. Session control: stream, wait, download

Sessions are the low-level API. The handle a session gives you can do everything
to itself — stream its events, wait for it to park, and download its record:

```ts
const session = await aex.openSession({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

// A turn streams its own events; iterate them, then collect the result.
const turn = session.send("Write a short report and save it as a file.");
for await (const event of turn) {
  console.log(event.type);
}
await turn.done();

// Reads/streams/downloads are grouped into accessor sub-resources:
// session.messages() / events() / outputs() / webhooks(). Grab the last
// assistant message (an AssistantTextEntry; use ?.text for the string).
const lastText = (await session.messages().last())?.text;
console.log(lastText);

// Poll the record until the session parks (idle / suspended / error).
const record = await session.wait();
console.log(record.status);

// Download the whole session record (metadata, events, outputs) as a zip.
await session.download({ to: "./session.zip" });
```

The same run from the bundled CLI:

```bash
aex run \
  --api-token "$AEX_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write a short report and save it as a file." \
  --follow
```

## Add capabilities

- Add files, skills, AGENTS.md, MCP servers, proxy endpoints, packages, and networking controls with [Composition](concepts/composition.md).
- Inspect runtime tools with [Agent tools](concepts/agent-tools.md).
- Use parent/child run delegation from the [Features](https://aex.dev/docs/features/#subagents) page.
- Narrow output capture or download individual files with [Outputs](outputs.md).
- Check supported providers and models in the [provider/runtime capability matrix](provider-runtime-capabilities.md).
