---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: "Overview"
description: "Explicit durable agent sessions through the strict aex v1 API."
---

# aex

Explicit durable agent sessions through the strict aex v1 API.

Create sessions, admit messages and runs, manage overwrite-by-name workspace resources, read persisted or live files through grants, and query or export telemetry.

## Feature areas

- **Sessions and runs.** Explicit session creation, message admission, durable run polling, and operation-backed lifecycle changes.
- **Registered resources and files.** Overwrite-by-name inputs plus distinct persisted and live file reads with short-lived download grants.
- **Telemetry.** Query, stream, aggregate, inspect gaps, and create durable telemetry exports.
- **Account and billing.** Bootstrap resources for accounts, organizations, workspaces, API keys, balances, usage, and statements.

## First run

### TypeScript

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_WORKSPACE_API_KEY!);
const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5"
});
const { run } = await session.messages.send("Summarize this repository.");
const result = await run.result();
console.log(result.id, result.status);
```

### CLI

```bash
bun add --global @aexhq/cli
aex sessions create --request @session.json --api-key "$AEX_WORKSPACE_API_KEY"
```

## Next

- [Quickstart](/docs/guides/quickstart/)
- [Features](/docs/features/)
- [Composition](/docs/concepts/composition/)
