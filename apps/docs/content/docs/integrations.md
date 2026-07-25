---
title: "Integrations"
description: "Public integration points for models, MCP servers, skills, files, and webhooks."
---

# Integrations

aex keeps integrations explicit in the SDK call site so sessions are reproducible and auditable.

## Models

Model access is managed. Name a model by its `creator/model` gateway slug and the platform's key routes it — you supply no provider API key. Slug shape and runtime pairings are described in [Model access](/docs/reference/provider-runtime-capabilities/).

```ts
await aex.start({
  model: "anthropic/claude-haiku-4-5",
  message: "Summarize the latest local benchmark output."
});
```

## MCP servers

Register workspace MCP servers in the dashboard, then reference them by id from code. Use [MCP](/docs/guides/mcp/) for the end-to-end shape.

## Skills, files, and AGENTS.md

Attach reusable behavior with [skills](/docs/guides/skills/), mount files with [run configuration](/docs/guides/session-config/), and include AGENTS.md context with [composition](/docs/concepts/composition/).

## Webhooks and files

Use [webhooks](/docs/guides/webhooks/) for terminal callbacks and [files](/docs/guides/files/) for captured files, links, downloads, and search.
