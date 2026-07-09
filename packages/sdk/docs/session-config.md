---
title: SessionRecord configuration
---

# SessionRecord configuration

A session config is the credential-free subset of the session options (`openSession` / `run`) that you can keep in code or load from a JSON file. It is not a platform object, saved definition, DSL, trigger, or persistent agent profile. aex only stores the immutable session record created when the session lands.

Allowed fields:

- `model` - required.
- `system` - optional system message.
- `mcpServers` - array of `McpServerRef`; headers are split into the vaulted secrets channel server-side.
- `environment` - `{ networking?, packages?, variables? }`. Networking is open by default; set `networking.mode` to `limited` only when you want an allowlist. `variables` are merged into the in-container `RUNTIME.env` / `RUNTIME.json` mounts. (SessionRecord secrets go in `environment.secrets`, which carries live `Secret` instances and is not part of a shareable config.)
- `runtime` - optional managed-runtime preset. Prefer `Sizes` in TypeScript.
- `metadata` - non-secret structured metadata.
- `overrides` - `{ idleTtl?, timeout?, maxSpendUsd?, maxTurns? }`. `timeout` is an optional session deadline (e.g. `"30m"`, `"2h"`); `maxSpendUsd` stops the session once its spend would exceed the cap; `maxTurns` caps the agent's iteration count for the turn (a positive integer — omit to accept the platform default of 20, clamped to the ceiling). See [Limits & quotas](limits-and-quotas.md).

`message` (the one-shot `run` input), `agentsMd`, `files`, `outputs`, `tools`, `skills`, `includeBuiltinTools`, and `outputMode` are `openSession` / `run` options, not reusable session-config fields. They carry the turn input, bytes, capture behavior, or agent tool/output controls that belong on a concrete call. Skill bundles are `skills` entries built with `Skill.fromDir(...)`, `Skill.fromUrl(...)`, or the other `Skill.from*` factories, so they too are SDK-code options rather than config fields. Subagents are session-internal (the in-run `subagent` tool — see [Subagents](concepts/subagents.md)); there is no `parentSessionId` option. The wire contract carries a per-session `limits` object (the exported `SessionLimits` type: `maxConcurrentChildSessions`, `maxSubagentDepth`, `maxSpendUsd`, `maxTurns`), and the session surface exposes its spend and iteration dials via `overrides.maxSpendUsd` and `overrides.maxTurns`; the subagent depth/breadth dials are not settable per-session today and take the platform defaults.

Secrets never live in session config. Pass provider keys through the top-level
`apiKeys` map and runtime secrets through `environment.secrets` in the SDK, or
the equivalent host-mode flags (`--anthropic-api-key`, `--mcp-auth`) in the CLI.
See [Secrets](secrets.md) for secret lifecycles and [Credentials](credentials.md)
for credential handling.

## Reuse in code

Use an ordinary function when you want reusable typed parameters. aex does not store or execute this function; it only receives the parameters you pass.

```ts
import { Models } from "@aexhq/sdk";

function summarise(topic: string) {
  return {
    model: Models.CLAUDE_HAIKU_4_5,
    system: "You are a concise automation agent.",
    message: `Write a short answer about ${topic}.`
  };
}

await aex.start({
  ...summarise("agent-first SDK design"),
  apiKeys: { anthropic: apiKey }
});
```

## CLI

The `aex start` host subcommand accepts the same session config either as a JSON file:

```bash
aex start --config ./main.json \
  --api-key "$AEX_API_KEY" \
  --anthropic-api-key "$ANTHROPIC_API_KEY"
```

...or as explicit flags (`--model`, `--system`, `--prompt`, `--mcp`, `--mcp-auth`, `--runtime-size`, `--session-timeout`, `--metadata`). The two modes are mutually exclusive. Composition inputs attach with repeatable flags that map onto the SDK's `Skill`/`Tool`/`AgentsMd`/`File` factories: `--skill @bundle` (workspace skill), `--tool @tool.js` (custom tool module), `--agents-md @AGENTS.md`, and `--file @path` (mount a file into `/workspace`).
