---
title: aex
---

# aex

aex is a TypeScript-first SDK + CLI for running autonomous agent sessions across providers through the managed aex service. Everything an agent or human needs is reachable through **one** import and **one** binary.

```ts
import {
  AexClient,        // the only client class — submits durable runs to aex
  Skill,                // workspace / provider / inline skill bundles
  McpServer,            // MCP server declarations (headers split into secrets server-side)
  ProxyEndpoint,        // per-run managed HTTP proxy endpoint
  AgentsMd,             // AGENTS.md / CLAUDE.md uploads
  File,                 // arbitrary workspace files mounted into the session
  validateProxyAuth     // helper that fails fast on policy/auth mismatch
} from "@aexhq/sdk";
```

```bash
aex run     --config ./run.json --api-token ant_… \
                --anthropic-api-key sk-ant-… --follow
aex status  <run-id>             --api-token …
aex wait    <run-id> [--timeout 8m] [--interval 2s] --api-token …
aex events  <run-id> [--follow] [--timeout 8m]  --api-token …
aex outputs <run-id>             --api-token …
aex download <run-id> [--only outputs|logs|events|metadata] [--out path] --api-token …
aex cancel  <run-id>             --api-token …
aex delete  <run-id>             --api-token …
aex whoami                       --api-token …
aex skills  <upload|list|get|delete> [flags] --api-token …
```

The SDK class and the CLI are backed by the same public `@aexhq/contracts` operations module — any read or write you can do through one, you can do through the other, against the same durable run records. The same npm package also ships the in-container `aex` CLI as its `bin` entry; managed runs mount that CLI inside the runner so skills can call `aex proxy …` against the per-run manifest. See [product capabilities and boundaries](docs/product-boundaries.md).

The aex URL defaults to `https://api.aex.dev`. Set `--aex-url` on the CLI or `baseUrl` on `AexClient` for local, staging, or hosted aex API planes. This is not a supported self-host deployment claim. The workspace is derived server-side from your API token (1:1 binding), so there is no `--workspace` flag and no `workspaceId` option.

## Product boundaries

- Multi-provider via Goose Managed. The published surface is the same
  regardless of provider:
  - omit `runtime` or pass `runtime: "managed"`; every provider uses the
    managed runtime and BYOK provider-proxy.
  - `provider: "anthropic" | "deepseek" | "openai" | "gemini" | "mistral"`.
  - See [provider/runtime capabilities](docs/provider-runtime-capabilities.md)
    for the generated matrix.
- BYO provider key + MCP credentials + skill references — passed inline on every submission and held in run-scoped custody, with cleanup/revocation attempted at terminal. Cross-provider keys are rejected loudly at submission time.
- Workspace is the tenant boundary. Workspace identity is derived server-side from the API token (1:1 binding); the SDK / CLI never name it.
- No SDK-side storage of provider keys, MCP credentials, or output file contents.
- Cleanup runs by default for tracked runtime resources.
- Product boundaries are explicit: see [product capabilities and boundaries](docs/product-boundaries.md) for what aex owns, inherits, and does not support.

## Quickstart (SDK)

```ts
import { AexClient } from "@aexhq/sdk";

const client = new AexClient({
  apiToken: process.env.AEX_API_TOKEN!
  // baseUrl defaults to https://api.aex.dev - set it for local or staging planes.
});

const runId = await client.submitRun({
  model: "claude-haiku-4-5",
  system: "You are a concise automation agent.",
  prompt: "Write a short answer about agent-first SDK design.",
  secrets: { anthropic: { apiKey: process.env.ANTHROPIC_API_KEY! } }
});

const run = await client.wait(runId);
console.log(run.status);

for (const output of await client.outputs(runId)) {
  console.log(output.id, output.filename);
}

const report = await client.downloadOutput(runId, { path: "report.txt", match: "suffix" });
console.log(new TextDecoder().decode(report));

await client.downloadOutputs(runId, { to: "./outputs.zip" });
```

Reusable, credential-free configs can be ordinary functions:

```ts
function summarise(topic: string) {
  return {
  model: "claude-haiku-4-5",
  system: "You are a concise automation agent.",
  prompt: `Write a short answer about ${topic}.`
  };
}

const runId = await client.submitRun({
  ...summarise("agent-first SDK design"),
  secrets: { anthropic: { apiKey: process.env.ANTHROPIC_API_KEY! } }
});
```

Stream events live with `client.stream(runId)`:

```ts
for await (const event of client.stream(runId)) {
  if (event.type === "agent.message") {
    // typed event helpers live under `aex`'s event guard exports.
  }
}
```

The same flow from the CLI (two equivalent forms):

```bash
aex run \
  --api-token "$AEX_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --system "You are a concise automation agent." \
  --prompt "Write a short answer about agent-first SDK design." \
  --follow

aex run \
  --api-token "$AEX_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --config ./run.json \
  --follow
```

`--config` accepts a plain run-config JSON file for a single run request: `{ model, system?, prompt, skills?, mcpServers?, environment?, proxyEndpoints?, metadata? }`. There is no saved-definition product or interpolation DSL — build the JSON at the call site.

## Test commands

```text
pnpm test                # unit (deterministic; uses fakes/snapshots)
pnpm test:user:offline   # clean install of packed/published SDK, no live API
pnpm test:user           # live hosted API user tests
pnpm test:user:heavy     # explicit heavy live canary
```

User tests auto-pack the current SDK when no artifact env is set. CI can pin a
specific artifact with exactly one of `AEX_USER_TEST_TARBALL` or
`AEX_USER_TEST_VERSION`; live runs also need `AEX_API_URL`,
`AEX_API_TOKEN`, and `DEEPSEEK_API_KEY`.

## Guides

- [Quickstart](docs/quickstart.md)
- [Run config](docs/run-config.md)
- [Run record](docs/run-record.md)
- [Product capabilities and boundaries](docs/product-boundaries.md)
- [Provider/runtime capabilities](docs/provider-runtime-capabilities.md)
- [Credentials](docs/credentials.md)
- [MCP](docs/mcp.md)
- [Skills](docs/skills.md)
- [Outputs](docs/outputs.md)
- [Events](docs/events.md)
- [Cleanup](docs/cleanup.md)
- [Testing](docs/testing.md)
- [Release](docs/release.md)

