---
title: Overview
description: TypeScript SDK and CLI docs for the serverless control plane for autonomous agent sessions.
icon: BookOpenText
---

aex is the serverless control plane for autonomous agent sessions. Declare
the agent's environment — model, prompt, skills, MCP servers, files, and
optional output-capture roots — submit it, and get back a typed event stream
plus captured outputs.

One submission shape works across every supported provider:

```ts
import { AgentExecutor, RunModels } from "@aexhq/sdk";

const aex = new AgentExecutor({ apiToken: process.env.AEX_API_TOKEN! });

const runId = await aex.submitRun({
  model: RunModels.CLAUDE_HAIKU_4_5,
  prompt: "Summarise Q1 revenue by region.",
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});

for await (const event of aex.stream(runId)) console.log(event.type);
const run = await aex.wait(runId);
await aex.download(runId, { to: "./run.zip" });
```

What you get:

- **One multi-provider surface.** The same `submitRun` shape and event stream
  for Anthropic, DeepSeek, OpenAI, Gemini, and Mistral. Anthropic and DeepSeek
  are live-verified; OpenAI, Gemini, and Mistral are accepted but not yet
  live-verified — see the
  [provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/)
  for per-provider status.
- **BYOK custody.** Provider keys travel inline per run, are held in
  run-scoped custody, and are excluded from idempotency hashing.
- **An ordered, durable event stream.** Every run emits one typed event
  shape, recorded after secret redaction and readable live or after the fact.
- **Cleanup by default.** Tracked runtime resources are reclaimed at terminal,
  with `cleanupStatus` surfacing anything that could not complete.

New here? Read [Why aex / how it compares](/docs/why-aex/), then run the
[Quickstart](/docs/guides/quickstart/).
