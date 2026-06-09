---
title: Run configuration
---

# Run configuration

A run config is the credential-free subset of a `submitRun` request that you can keep in code or load from a JSON file. It is not a platform object, saved definition, DSL, trigger, or persistent agent profile. aex only stores the immutable run record created when you submit.

Allowed fields:

- `model` - required.
- `prompt` - required, string or array of strings.
- `system` - optional system message.
- `skills` - array of storage-neutral `kind:"asset"` refs. Config files cannot carry local draft bytes; use `Skill.fromPath(...)` / `Skill.fromFiles(...)` in SDK code first, or reference an existing asset/catalog skill.
- `mcpServers` - array of `McpServerRef`; headers are split into `secrets.mcpServers` server-side.
- `environment` - `{ networking?, packages?, envVars? }`. `envVars` are merged into the in-container `RUNTIME.env` / `RUNTIME.json` mounts.
- `runtimeSize` - optional managed-runtime preset. Prefer `RuntimeSizes` in TypeScript.
- `timeout` - optional run deadline duration string such as `"30m"` or `"2h"`.
- `proxyEndpoints` - array of `PlatformProxyEndpoint`.
- `metadata` - non-secret structured metadata.

`agentsMd`, `files`, `outputs`, `builtins`, and `outputMode` are top-level `submitRun` options, not run-config fields. They carry bytes, capture behavior, or runtime execution controls that belong on a concrete run submission.

Secrets never live in run config. Pass credentials through `submitRun({ ...config, secrets })` in the SDK or the equivalent host-mode flags (`--anthropic-api-key`, `--mcp-auth`, `--proxy-auth`) in the CLI.

## Reuse in code

Use an ordinary function when you want reusable typed parameters. aex does not store or execute this function; it only receives the run parameters you submit.

```ts
import { RunModels } from "@aexhq/sdk";

function summarise(topic: string) {
  return {
    model: RunModels.CLAUDE_HAIKU_4_5,
    system: "You are a concise automation agent.",
    prompt: `Write a short answer about ${topic}.`
  };
}

await aex.submitRun({
  ...summarise("agent-first SDK design"),
  secrets: { apiKey }
});
```

## CLI

The `aex run` host subcommand accepts the same run config either as a JSON file:

```bash
aex run --config ./run.json \
  --api-token "$AEX_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY"
```

...or as explicit flags (`--model`, `--system`, `--prompt`, `--mcp`, `--mcp-auth`, `--runtime-size`, `--run-timeout`, `--proxy-endpoint`, `--proxy-auth`, `--metadata`). The two modes are mutually exclusive.
