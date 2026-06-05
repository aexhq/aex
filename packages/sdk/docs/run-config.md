---
title: Run configuration
---

# Run configuration

A run config is the credential-free subset of a `submitRun` request that you can keep in code or load from a JSON file. It is not a platform object, saved definition, DSL, trigger, or persistent agent profile. aex only stores the immutable run record created when you submit.

Allowed fields:

- `model` - required.
- `prompt` - required, string or array of strings.
- `system` - optional system message.
- `skills` - array of `SkillRef`, either workspace, provider, or inline.
- `mcpServers` - array of `McpServerRef`; headers are split into `secrets.mcpServers` server-side.
- `proxyEndpoints` - array of `PlatformProxyEndpoint`.
- `environment` - `{ networking?, packages?, envVars? }`. `envVars` are merged into the in-container `RUNTIME.env` / `RUNTIME.json` mounts.
- `metadata` - non-secret structured metadata.

`agentsMd`, `files`, and `outputs` are top-level `submitRun` options, not run-config fields. They carry bytes or capture behavior that belongs on a concrete run submission.

Secrets never live in run config. Pass credentials through `submitRun({ ...config, secrets })` in the SDK or the equivalent host-mode flags (`--anthropic-api-key`, `--mcp-auth`, `--proxy-auth`) in the CLI.

## Reuse in code

Use an ordinary function when you want reusable typed parameters. aex does not store or execute this function; it only receives the run parameters you submit.

```ts
function summarise(topic: string) {
  return {
  model: "claude-haiku-4-5",
  system: "You are a concise automation agent.",
  prompt: `Write a short answer about ${topic}.`
  };
}

await aex.submitRun({
  ...summarise("agent-first SDK design"),
  secrets: { anthropic: { apiKey } }
});
```

## CLI

The `aex run` host subcommand accepts the same run config either as a JSON file:

```bash
aex run --config ./run.json \
  --api-token "$AEX_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY"
```

...or as explicit flags (`--model`, `--system`, `--prompt`, `--mcp`, `--mcp-auth`, `--proxy-endpoint`, `--proxy-auth`, `--metadata`). The two modes are mutually exclusive.
