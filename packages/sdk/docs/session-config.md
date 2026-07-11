---
title: Session configuration
---

# Session configuration

`aex.sessions.create(...)` accepts the durable session configuration. The
one-shot `aex.start(...)` accepts the same fields plus `message`,
`messageIdempotencyKey`, `deleteAfter`, and stream options.

Core fields include:

- `model` and optional `provider`
- `system`
- immutable workspace refs grouped under `assets`
- `builtinTools`
- `mcpServers`
- `environment`
- `fileCapture`
- `runtime`, `metadata`, and `overrides`
- `outputMode`, `responseFormat`, `approvalGate`, and `webhook`
- `apiKeys` and `idempotencyKey`

Secrets are never part of a reusable JSON config. Supply provider credentials
through `apiKeys` and runtime secrets through `environment.secrets` at the call
site.

```ts
const base = {
  model: Models.CLAUDE_HAIKU_4_5,
  system: "You are a concise automation agent.",
  builtinTools: "default" as const,
  overrides: { idleTtl: "3m", timeout: "30m", maxTurns: 20 }
};

const session = await aex.sessions.create({
  ...base,
  assets: { files: [input], instructions: [rules] },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});
```

`assets` accepts only refs returned by `aex.workspace.files`, `skills`, `tools`,
and `instructions`. Publish local drafts before create; there is no implicit
upload or compatibility field.

## CLI

`aex start` accepts a credential-free JSON config through `--config`, or
explicit flags such as `--model`, `--system`, `--prompt`, `--mcp`,
`--runtime-size`, and `--session-timeout`. Reusable resources are published and
attached with repeatable `--skill`, `--tool`, `--instructions`, and `--file`
flags. The legacy instruction flag is not accepted.
