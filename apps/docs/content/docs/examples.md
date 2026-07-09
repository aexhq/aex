---
title: "Examples"
description: "Runnable public patterns for common aex workflows."
---

# Examples

Use these as starting points for local projects. Each example keeps provider keys in your environment and submits through the SDK or CLI.

## Quick agent run

```ts
import { Aex, Models, Sizes } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);

const result = await aex.start({
  model: Models.CLAUDE_HAIKU_4_5,
  runtime: Sizes.SHARED_0_25X_1GB,
  system: "You are a concise engineering assistant.",
  message: "Inspect this repository and list the highest-priority fix.",
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

console.log(result.status, result.text);
```

## Session with a follow-up turn

```ts
const session = await aex.openSession({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

await session.send("Create a short plan.").done();
const final = await session.send("Now apply the first step.").done();
console.log(final.text);
```

## CLI smoke test

```bash
aex start \
  --api-key "$AEX_API_KEY" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write a short report and save it as outputs/report.md" \
  --follow
```

Next: [Quickstart](/docs/guides/quickstart/), [Composition](/docs/concepts/composition/), and [CLI reference](/docs/reference/cli/).
