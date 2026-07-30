---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs` from `packages/sdk/docs/concepts/composition.md`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: Composition
---

# Composition

Compose a session from registered names and explicit request policy:

```ts
const session = await aex.sessions.create({
  model: "openai/gpt-5",
  registered: {
    files: ["repository-context"],
    instructions: ["review-policy"],
    tools: ["release-check"]
  },
  credentials: {
    secrets: [{ name: "github-token" }]
  },
  network: {
    hands: { mode: "public_internet" }
  }
});
```

The session record exposes the resolved configuration and continuity state.
Mutations such as persist, stop, fork, workspace discard, credential rebind,
and delete are explicit durable operations.
