---
title: "Integrations"
description: "Public integration points for models, MCP servers, registered resources, files, and telemetry."
---

# Integrations

aex keeps external access and reusable workspace inputs explicit.

## Models

Model access is managed. Create a session with its `creator/model` gateway slug;
you do not supply a provider API key.

```ts
const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5"
});
```

## MCP servers

Register an MCP server under a workspace name, then include that name when the
session is created.

```ts
const session = await aex.sessions.create({
  model: "anthropic/claude-haiku-4-5",
  registered: { mcpServers: ["github"] }
});
```

Use [MCP](/docs/guides/mcp/) for the end-to-end shape.

## Files, skills, tools, and instructions

Registered resources are addressed by name. Setting the same name replaces its
current value; a new session copies the selected current values into its own
workspace.

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

See [registered resources](/docs/guides/resources/), [files](/docs/guides/files/),
and [composition](/docs/concepts/composition/).

## Observability

Use events, logs, spans, metrics, or traces independently, or use telemetry to
query, stream, listen to, and export all signals together. See
[telemetry](/docs/guides/telemetry/).
