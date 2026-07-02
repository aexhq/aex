---
title: Run configuration
---

# Run configuration

A run config is the credential-free subset of the session options (`openSession` / `run`) that you can keep in code or load from a JSON file. It is not a platform object, saved definition, DSL, trigger, or persistent agent profile. aex only stores the immutable session record created when the session lands.

Allowed fields:

- `model` - required.
- `system` - optional system message.
- `mcpServers` - array of `McpServerRef`; headers are split into the vaulted secrets channel server-side.
- `environment` - `{ networking?, packages?, variables? }`. Networking is open by default; set `networking.mode` to `limited` only when you want an allowlist. `variables` are merged into the in-container `RUNTIME.env` / `RUNTIME.json` mounts. (Run secrets go in `environment.secrets`, which carries live `Secret` instances and is not part of a shareable config.)
- `runtime` - optional managed-runtime preset. Prefer `Sizes` in TypeScript.
- `metadata` - non-secret structured metadata.
- `overrides` - `{ idleTtl?, timeout?, maxSpendUsd? }`. `timeout` is an optional session deadline (e.g. `"30m"`, `"2h"`); `maxSpendUsd` stops the session once its spend would exceed the cap (see [Limits & quotas](limits-and-quotas.md)).

`message` (the one-shot `run` input), `agentsMd`, `files`, `outputs`, `tools`, `includeBuiltinTools`, and `outputMode` are `openSession` / `run` options, not reusable run-config fields. They carry the turn input, bytes, capture behavior, or agent tool/output controls that belong on a concrete call. Skill bundles are `tools` entries built with `Tools.fromSkillDir(...)` / `Tools.fromSkillUrl(...)`, so they too are SDK-code options rather than config fields. Subagents run in-process; there is no `limits` / `parentRunId` option.

Secrets never live in run config. Pass provider keys through the top-level
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

await aex.run({
  ...summarise("agent-first SDK design"),
  apiKeys: { anthropic: apiKey }
});
```

## CLI

The `aex run` host subcommand accepts the same run config either as a JSON file:

```bash
aex run --config ./run.json \
  --api-token "$AEX_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY"
```

...or as explicit flags (`--model`, `--system`, `--prompt`, `--mcp`, `--mcp-auth`, `--runtime-size`, `--run-timeout`, `--metadata`). The two modes are mutually exclusive.
