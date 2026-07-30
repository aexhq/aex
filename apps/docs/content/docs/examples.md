---
title: "Examples"
description: "Runnable public patterns for common aex workflows."
---

# Examples

Use these as starting points for local projects. The canonical
[`aexhq/examples`](https://github.com/aexhq/examples) repository contains
complete runnable projects. Each example needs only an aex workspace key.

## Create a session and run one message

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_WORKSPACE_API_KEY!);
const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5",
  compute: { size: "1gb" }
});
const { run } = await session.messages.send(
  "Inspect this repository and list the highest-priority fix.",
  { idempotencyKey: "inspect-priority" }
);
const result = await run.result();

console.log(result.status, result.outputMessageIds);
```

## Create a session from registered names

```ts
const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5",
  registered: {
    files: ["project-context"],
    skills: ["code-review"],
    instructions: ["repository-rules"]
  }
});
```

The selected current resources are copied into the new session workspace.

## CLI

```bash
aex sessions create \
  --api-key "$AEX_WORKSPACE_API_KEY" \
  --request @session.json

aex messages send "$SESSION_ID" \
  --api-key "$AEX_WORKSPACE_API_KEY" \
  --idempotency-key inspect-priority \
  --request '{"input":"Inspect this repository."}'
```

Next: browse the [example repository](https://github.com/aexhq/examples),
[Quickstart](/docs/guides/quickstart/), [Composition](/docs/concepts/composition/),
and [CLI reference](/docs/reference/cli/).
