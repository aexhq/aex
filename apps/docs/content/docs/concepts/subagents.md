---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs` from `packages/sdk/docs/concepts/subagents.md`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: Subagents
description: Delegate bounded work to child sessions and inspect their lineage.
icon: GitFork
---

The builtin `subagent` capability lets an agent delegate work to child sessions.
Enable it with the default builtin set or include it explicitly in
`builtinTools`. Use `builtinTools: "none"` to disable agent-driven delegation.

Children are durable session records with their own events and checkpointed
files. Discover them from the parent handle:

```ts
const children = await session.children();

for (const child of children) {
  console.log(child.id, child.parentSessionId, child.depth, child.status);
  const events = await child.events.list();
  const snapshot = await child.files.list();
  const descendants = await child.children();
  console.log(child.ref.lastRun?.outcome, events.length, snapshot.files.length, descendants.length);
}
```

Child `status` is a session lifecycle state. Inspect `lastRun.outcome` or the
child's terminal RUN event for its run verdict. Child handles are read-only:
they expose events, checkpointed files, and recursive lineage, but not top-level
session controls such as send, cancel, suspend, resume, or delete.

Provider credentials are inherited through the hosted runtime's vaulted
channel; they are not copied into public child submissions or event payloads.
Depth, breadth, and spend limits are enforced server-side. Use
`overrides.maxSpendUsd` and a deliberate builtin selection to bound a parent
workflow.
