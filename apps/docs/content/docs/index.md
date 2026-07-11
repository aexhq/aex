---
title: "Overview"
description: "aex is an agent execution platform for launching autonomous agents from a simple TypeScript SDK and CLI."
---

# aex

aex is an agent execution platform for launching autonomous agents from a simple TypeScript SDK and CLI.

Open durable agent sessions, send turns, stream events, capture files, and compose agents with skills, files, MCP, secrets, networking controls, and subagents across the managed runtime.

## Feature areas

- **Agent runtime.** Managed autonomous sessions with filesystem read/edit, grep/glob/head/tail, open web fetch/search, background commands, code execution, git, and subagents.
- **Durable infrastructure.** Resumable session lifecycle, explicit run outcomes, committed checkpoints, idempotency, typed events, file capture, downloads, timeouts, and runtime sizes.
- **Agent composition.** Version-pinned skills, files, custom tools, instructions, remote MCP servers, environment variables, secrets, and networking controls.
- **Subagents.** Typed parent/child lineage for async child sessions, file handoff, and bounded agent delegation.
- **Models and providers.** Anthropic, DeepSeek, OpenAI, Gemini, Mistral, OpenRouter, and Doubao behind one submission shape.
- **Typed control surface.** Strongly typed SDK inputs, CLI parity, BYOK provider keys, workspace secrets, redaction, assistant text modes, and file capture policy.

## First run

### TypeScript

```ts
import { Aex, Models, Sizes } from "@aexhq/sdk";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });

const session = await aex.sessions.create({
  model: Models.CLAUDE_HAIKU_4_5,
  system: "You are a concise engineering assistant.",
  runtime: Sizes.SHARED_0_25X_1GB,
  overrides: { idleTtl: "3m" },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

const result = await session.messages.send("Write a short report and save it as a file.").finished();
console.log(result.status, result.text);
```

### CLI

```bash
aex start \
  --api-key "$AEX_API_KEY" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write a short report and save it as a file." \
  --follow
```

## Next

- [Quickstart](/docs/guides/quickstart/)
- [Features](/docs/features/)
- [Composition](/docs/concepts/composition/)
- [Provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/)
