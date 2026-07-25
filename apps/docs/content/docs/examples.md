---
title: "Examples"
description: "Runnable public patterns for common aex workflows."
---

# Examples

Use these as starting points for local projects. The canonical
[`aexhq/examples`](https://github.com/aexhq/examples) repository contains
complete runnable projects, including the vision-skill example. Each example
needs only your aex workspace key: model access is managed, so you name a model
by its `creator/model` gateway slug and supply no provider API key.

## Quick agent run

```ts
import { Aex, Sizes } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);

const result = await aex.start({
  model: "anthropic/claude-haiku-4-5",
  runtime: Sizes.CPU_0_25_1GB,
  system: "You are a concise engineering assistant.",
  message: "Inspect this repository and list the highest-priority fix."
});

console.log(result.status, result.text);
```

## Session with a follow-up turn

```ts
const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5"
});

await session.messages.send("Create a short plan.").finished();
const final = await session.messages.send("Now apply the first step.").finished();
console.log(final.text);
```

## CLI smoke test

```bash
aex start \
  --api-key "$AEX_API_KEY" \
  --model anthropic/claude-haiku-4-5 \
  --prompt "Write a short report and save it as files/report.md" \
  --follow
```

Next: browse the [example repository](https://github.com/aexhq/examples),
[Quickstart](/docs/guides/quickstart/), [Composition](/docs/concepts/composition/),
and [CLI reference](/docs/reference/cli/).
