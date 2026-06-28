---
title: Run configuration
---

# Run configuration

A run config is the credential-free subset of a `submit` request that you can keep in code or load from a JSON file. It is not a platform object, saved definition, DSL, trigger, or persistent agent profile. aex only stores the immutable run record created when you submit.

Allowed fields:

- `model` - required.
- `prompt` - required, string or array of strings.
- `system` - optional system message.
- `skills` - array of storage-neutral `kind:"asset"` refs. Config files cannot carry local draft bytes; use `Skill.fromPath(...)` / `Skill.fromFiles(...)` in SDK code first, or reference an existing asset/catalog skill.
- `mcpServers` - array of `McpServerRef`; headers are split into `secrets.mcpServers` server-side.
- `environment` - `{ networking?, packages?, envVars? }`. Networking is open by default; set `networking.mode` to `limited` only when you want an allowlist. `envVars` are merged into the in-container `RUNTIME.env` / `RUNTIME.json` mounts.
- `runtimeSize` - optional managed-runtime preset. Prefer `RuntimeSizes` in TypeScript.
- `region` - optional product placement region: `eu-west`, `us-west`, or `ap-northeast`. These are platform placement targets, not exact city guarantees; omitted runs infer a configured region from request geography and fall back when no hint matches.
- `timeout` - optional run deadline duration string such as `"30m"` or `"2h"`.
- `postHook` - optional post-agent verifier `{ command, timeout?, maxTurns?, maxChars? }`. It runs after a successful agent process; a failing or timed-out command is sent back to the agent for repair until `maxTurns` is exhausted. Empty `command` is treated as omitted.
- `proxyEndpoints` - array of `PlatformProxyEndpoint`; endpoint-level `retry` is allowed here and remains declaration-based.
- `metadata` - non-secret structured metadata.

`agentsMd`, `files`, `outputs`, `tools`, `includeBuiltinTools`, `limits`, and `outputMode` are top-level `submit` options, not run-config fields. They carry bytes, capture behavior, or agent tool/output controls that belong on a concrete run submission. The `limits` option sets per-run caps: the subagent-lineage caps (`maxConcurrentChildRuns`, `maxSubagentDepth`) and a USD spend cap (`maxSpendUsd`, which stops the run once its spend would exceed the cap); see [Limits & quotas](limits-and-quotas.md).

Secrets never live in run config. Pass credentials through `submit({ ...config, secrets })` in the SDK or the equivalent host-mode flags (`--anthropic-api-key`, `--mcp-auth`, `--proxy-auth`) in the CLI. See [Secrets](secrets.md) for secret lifecycles and [Credentials](credentials.md) for the proxy endpoint policy/auth split and retry fields.

When a run uses `postHook`, the terminal event includes `data.postHook` with
attempt counts, the final hook result, and capped failure output. A hook that
exhausts `maxTurns` fails the run with `data.failureClass: "post_hook_failed"`.

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

await aex.submit({
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

...or as explicit flags (`--model`, `--system`, `--prompt`, `--mcp`, `--mcp-auth`, `--region`, `--runtime-size`, `--run-timeout`, `--proxy-endpoint`, `--proxy-auth`, `--metadata`). The two modes are mutually exclusive. `postHook` is available through `--config`; there are no standalone hook flags.
