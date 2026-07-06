---
title: "Integrations"
description: "Public integration points for providers, MCP servers, skills, files, webhooks, and outputs."
---

# Integrations

aex keeps integrations explicit in the SDK call site so runs are reproducible and auditable.

## Providers

Pass BYOK provider keys per run. Supported provider/model combinations are listed in the [provider/runtime matrix](/docs/reference/provider-runtime-capabilities/).

```ts
await aex.run({
  model: "claude-haiku-4-5",
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! },
  message: "Summarize the latest local benchmark output."
});
```

## MCP servers

Register workspace MCP servers in the dashboard, then reference them by id from code. Use [MCP](/docs/guides/mcp/) for the end-to-end shape.

## Skills, files, and AGENTS.md

Attach reusable behavior with [skills](/docs/guides/skills/), mount files with [run configuration](/docs/guides/run-config/), and include AGENTS.md context with [composition](/docs/concepts/composition/).

## Webhooks and outputs

Use [webhooks](/docs/guides/webhooks/) for terminal callbacks and [outputs](/docs/guides/outputs/) for captured files, links, downloads, and search.
