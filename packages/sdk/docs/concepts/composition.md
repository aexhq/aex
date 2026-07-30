---
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
