---
title: Quickstart
---

# Quickstart

## 1. Install

```bash
npm i @aexhq/sdk
```

This installs the TypeScript SDK exports and the bundled `aex` CLI.

## 2. Set credentials

aex is currently in **invite-only beta**: workspaces and API keys are issued
by the aex team — contact <support@aex.dev> for beta access. Once you have
access, create a quickstart SDK token with `runs:read`, `runs:write`,
`outputs:read`, and `billing:read` in the dashboard at <https://aex.dev>. The
examples also need your BYOK provider key for the model you choose. For the
Claude examples below:

```bash
export AEX_API_KEY="<your-aex-api-key>"
export ANTHROPIC_API_KEY="<your-anthropic-api-key>"
```

## 3. Open a session

```ts
import { Aex, Models, Sizes } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);

const session = await aex.openSession({
  model: Models.CLAUDE_HAIKU_4_5,
  system: "You are a concise engineering assistant.",
  runtime: Sizes.SHARED_0_25X_1GB,
  overrides: { idleTtl: "3m" },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

const first = await session.send("Write a short report and save it as a file.").done();
console.log(first.status, first.costUsd, first.text);
```

`send().done()` (and `run()`) **await settle by default**, so the result always
carries a terminal `status` (`succeeded` / `failed` / `timed_out` / `cancelled`
— never a bare `idle`) plus `costUsd` and `usage`. Pass `await: 'park'` to
return early at the render-complete park event when you don't need cost/usage.
`costUsd` is aex runtime/storage spend and **excludes** your BYOK provider
charges — price those from `usage` token counts against your provider's rates.

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

const messages = await session.messages().list();
console.log(messages.at(-1)?.text);

// Poll the record until the session parks: a resumable `idle` / `suspended`,
// or a terminal outcome (`succeeded` / `failed` / `timed_out` / `cancelled`).
const record = await session.wait();
console.log(record.status);

// Download the whole session record (metadata, events, outputs) as a zip.
await session.download({ to: "./session.zip" });
```

The same run from the bundled CLI (`npx aex` on a local install; or
`npm i -g @aexhq/sdk` for a bare `aex`):

```bash
npx aex run \
  --api-key "$AEX_API_KEY" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write a short report and save it as a file." \
  --follow
```

## Add capabilities

- Add files, skills, AGENTS.md, MCP servers, packages, and networking controls with [Composition](concepts/composition.md).
- Delegate bounded sub-tasks to child runs with [Subagents](concepts/subagents.md).
- Get notified when a run finishes with [Webhooks](webhooks.md).
- Narrow output capture or download individual files with [Outputs](outputs.md).
- Check supported providers and models in the [provider/runtime capability matrix](provider-runtime-capabilities.md).
