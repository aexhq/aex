---
title: Quickstart
---

# Quickstart

## 1. Install

```bash
npm install @aexhq/sdk
```

## 2. Submit a run

```ts
import { AgentExecutor, Models, Providers } from "@aexhq/sdk";

const aex = new AgentExecutor({
  apiToken: process.env.AEX_API_TOKEN!
});

const runId = await aex.submit({
  provider: Providers.ANTHROPIC,
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "Write a short report and save it as a file.",
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});
```

## 3. Stream, wait, and download

```ts
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
