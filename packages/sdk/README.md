---
title: antpath
---

# antpath

antpath is a TypeScript-first SDK + CLI for running autonomous agent sessions across providers through the managed antpath service. Everything an agent or human needs is reachable through **one** import and **one** binary.

```ts
import {
  AntpathClient,        // the only client class — submits durable runs to antpath
  Skill,                // workspace / provider / inline skill bundles
  McpServer,            // MCP server declarations (headers split into secrets server-side)
  ProxyEndpoint,        // per-run managed HTTP proxy endpoint
  AgentsMd,             // AGENTS.md / CLAUDE.md uploads
  File,                 // arbitrary workspace files mounted into the session
  validateProxyAuth     // helper that fails fast on policy/auth mismatch
} from "antpath";
```

```bash
antpath run     --config ./run.json --api-token ant_… \
                --anthropic-api-key sk-ant-… --follow
antpath status  <run-id>             --api-token …
antpath wait    <run-id> [--timeout 8m] [--interval 2s] --api-token …
antpath events  <run-id> [--follow] [--timeout 8m]  --api-token …
antpath outputs <run-id>             --api-token …
antpath download <run-id> [--only outputs|logs|events|metadata] [--out path] --api-token …
antpath cancel  <run-id>             --api-token …
antpath delete  <run-id>             --api-token …
antpath whoami                       --api-token …
antpath skills  <upload|list|get|delete> [flags] --api-token …
```

The SDK class and the CLI are backed by the same public `@antpath/contracts` operations module — any read or write you can do through one, you can do through the other, against the same durable run records. The same npm package also ships the in-container `antpath` CLI as its `bin` entry; the worker mounts that CLI inside every run at `/mnt/session/uploads/antpath/antpath` (Anthropic Managed Agents rebases every session-resource mount under `/mnt/session/uploads/`, and mounted files have no execute permission so they are invoked through `node`), so skills can call `node /mnt/session/uploads/antpath/antpath proxy …` against the per-run manifest. See [Agent-first surface design](../docs/engineering.md).

The antpath URL defaults to `https://api.antpath.ai`. Set `--antpath-url` on the CLI or `baseUrl` on `AntpathClient` for local, staging, private, or hosted antpath API planes. This is not a supported self-host deployment claim. The workspace is derived server-side from your API token (1:1 binding), so there is no `--workspace` flag and no `workspaceId` option.

## Product boundaries

- Multi-provider via the dual-runtime architecture. The published surface is the same regardless of provider:
  - `provider: "anthropic"` — defaults to the Anthropic Native runtime
    unless you opt into Goose Managed with `runtime: "managed"`.
  - `provider: "deepseek" | "openai" | "gemini" | "mistral"` — Goose
    Managed runtime.
  - See [provider/runtime capabilities](docs/provider-runtime-capabilities.md)
    for the generated matrix.
- BYO provider key + MCP credentials + skill references — passed inline on every submission and held in run-scoped custody, with cleanup/revocation attempted at terminal. Cross-provider keys are rejected loudly at submission time.
- Workspace is the tenant boundary. Workspace identity is derived server-side from the API token (1:1 binding); the SDK / CLI never name it.
- No SDK-side storage of provider keys, MCP credentials, or output file contents.
- Cleanup runs by default; opt into retention with `cleanup.session: "retain"`.
- Product boundaries are explicit: see [product capabilities and boundaries](docs/product-boundaries.md) for what antpath owns, inherits, and does not support.

## Quickstart (SDK)

```ts
import { AntpathClient } from "antpath";

const client = new AntpathClient({
  apiToken: process.env.ANTPATH_API_TOKEN!
  // baseUrl defaults to https://api.antpath.ai - set it for local or staging planes.
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
    // typed event helpers live under `antpath`'s event guard exports.
  }
}
```

The same flow from the CLI (two equivalent forms):

```bash
antpath run \
  --api-token "$ANTPATH_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --system "You are a concise automation agent." \
  --prompt "Write a short answer about agent-first SDK design." \
  --follow

antpath run \
  --api-token "$ANTPATH_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --config ./run.json \
  --follow
```

`--config` accepts a plain run-config JSON file for a single run request: `{ model, system?, prompt, skills?, mcpServers?, environment?, cleanup?, proxyEndpoints?, metadata? }`. There is no saved-definition product or interpolation DSL — build the JSON at the call site.

## Test commands

```text
pnpm test                # unit (deterministic; uses fakes/snapshots)
pnpm test:e2e            # full top-to-bottom against a real antpath stack + Anthropic
pnpm test:user           # exercises the published package via npm install (offline + live)
```

`pnpm test:user` auto-packs the current SDK when no artifact env is set. CI can pin a specific artifact with exactly one of `ANTPATH_USER_TEST_TARBALL` or `ANTPATH_USER_TEST_VERSION` (+ live target vars for the live siblings).

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

