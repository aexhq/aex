---
title: "Overview"
description: "aex is an agent execution platform for launching autonomous agents from a simple TypeScript SDK and CLI."
---

# aex

aex is an agent execution platform for launching autonomous agents from a simple TypeScript SDK and CLI.

Open durable agent sessions, send turns, stream events, capture outputs, and compose agents with skills, files, MCP, proxy endpoints, and subagents across the managed runtime.

## Feature areas

- **Agent runtime.** Managed autonomous runs with filesystem read/edit, grep/glob/head/tail, open web fetch/search defaults, and optional notebook tools.
- **Durable infrastructure.** Run records, status, wait/cancel/delete, idempotency, typed events, output capture, downloads, timeouts, and runtime sizes.
- **Agent composition.** Skills, files, AGENTS.md, remote MCP servers, proxy endpoints, environment variables, packages, and networking controls.
- **Subagents.** Typed parent/child lineage for async child runs, output handoff, and bounded agent delegation.
- **Models and providers.** Anthropic, DeepSeek, OpenAI, Gemini, Mistral, OpenRouter, Doubao, and Doubao China behind one submission shape.
- **Typed control surface.** Strongly typed SDK inputs, CLI parity, BYOK secrets, scoped proxy auth, redaction, and output modes.

## First run

### TypeScript

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

const result = await session.send("Write a short report and save it as a file.").done();
console.log(result.status, result.text);
```

### CLI

```bash
aex run \
  --api-token "$AEX_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write a short report and save it as a file." \
  --follow
```

## Next

- [Quickstart](/docs/guides/quickstart/)
- [Features](/docs/features/)
- [Agent tools](/docs/concepts/agent-tools/)
- [Composition](/docs/concepts/composition/)
- [Provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/)
