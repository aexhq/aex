---
title: Quickstart
---

# Quickstart

## 1. Install

```bash
bun add @aexhq/sdk
```

## 2. Run a prompt

```ts
import { AgentExecutor, Models } from "@aexhq/sdk";

const aex = new AgentExecutor({ apiToken: process.env.AEX_API_TOKEN! });

// run() submits, waits for the run to settle, and returns the result.
// `provider` is derived from the model; `apiKey` is your BYOK provider key.
const { text, ok } = await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKey: process.env.ANTHROPIC_API_KEY!,
  prompt: "Write a short report and save it as a file."
});

console.log(ok, text);
```

## 3. Submit, stream, wait, and download

When you need the run id, live events, or downloads, drive the lifecycle yourself:

```ts
const runId = await aex.submit({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKey: process.env.ANTHROPIC_API_KEY!,
  prompt: "Write a short report and save it as a file."
});

for await (const event of aex.stream(runId)) {
  console.log(event.type);
}

const run = await aex.wait(runId);
console.log(run.status);

await aex.download(runId, { to: "./run.zip" });
```

The same run from the CLI:

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
