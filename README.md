# aex

[![npm version](https://img.shields.io/npm/v/@aexhq/sdk.svg)](https://www.npmjs.com/package/@aexhq/sdk)

> **Early development.** API and CLI surface are stable in intent but
> may shift. Pin a version and watch the GitHub releases page until
> we cut `1.0`.

`aex` is the serverless control plane for autonomous agent sessions.
Declare the agent's environment - model, prompt, skills, MCP servers,
files, output dirs - submit it, and get back a typed event stream plus
captured outputs. **BYOK** for provider keys; aex attempts cleanup
for tracked run resources at terminal while provider and infrastructure
retention stay under their own policies.

## Install

```bash
npm install @aexhq/sdk   # or: pnpm add @aexhq/sdk  /  yarn add @aexhq/sdk
```

## Example

```ts
import { AgentExecutor, RunModels, Skill, McpServer } from "@aexhq/sdk";

const aex = new AgentExecutor({ apiToken: process.env.AEX_API_TOKEN! });

const github = McpServer.remote({
  name: "github",
  url: "https://api.githubcopilot.com/mcp/",
  headers: { authorization: `Bearer ${process.env.GITHUB_TOKEN!}` },
});

const runId = await aex.submit({
  model: RunModels.CLAUDE_HAIKU_4_5,
  prompt: "Summarise Q1 revenue by region.",
  skills: [await Skill.fromPath("./skills/sheet-tools", { name: "sheet-tools" })],
  mcpServers: [github],
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! },
});

// Listen for events as the run executes...
for await (const event of aex.stream(runId)) console.log(event.type);

// ...then download the public run record — metadata, events, and
// captured outputs — as one zip once it finishes.
await aex.wait(runId);
await aex.download(runId, { to: "./run.zip" });
```

Same run request from the CLI: `aex run --config run.json` accepts
the same run-config fields (`{ model, prompt, skills, mcpServers, postHook, ... }`);
`aex events <run-id> --follow` streams, and
`aex wait <run-id> [--timeout 8m]` blocks until the run finishes.

## Composition

- **Skills, MCP servers, AGENTS.md, files** are first-class. Pass
  per-run bundles inline with `Skill.fromPath(...)` /
  `Skill.fromFiles(...)`; `aex.submit` materializes the bytes to
  content-addressable, workspace-scoped asset storage before the run lands,
  so the same bytes are a no-op upload on subsequent runs. MCP servers
  can be remote or workspace-registered.
- **Outputs are captured, tracked resource cleanup is attempted.** Managed
  runs capture files created or modified by the agent; `outputs.allowedDirs`
  can narrow that capture to specific roots. aex attempts cleanup of tracked runtime
  resources at terminal.

## Guides

- [Quickstart](packages/sdk/docs/quickstart.md) — install, auth, first run
- [Skills](packages/sdk/docs/skills.md) — inline, local, and catalog bundles normalized to assets
- [MCP servers](packages/sdk/docs/mcp.md) — remote servers, headers, workspace refs
- [Run config](packages/sdk/docs/run-config.md) — plain credential-free run parameters
- [Product capabilities and boundaries](packages/sdk/docs/product-boundaries.md) — what aex owns, inherits, and does not support
- [Credentials](packages/sdk/docs/credentials.md) — secrets bundle shape per provider
- [Provider/runtime capabilities](packages/sdk/docs/provider-runtime-capabilities.md) — supported providers and runtime routing
- [Outputs](packages/sdk/docs/outputs.md) — capture rules, retention, download
- [Events](packages/sdk/docs/events.md) — typed event guards, streaming
- [Cleanup](packages/sdk/docs/cleanup.md) — runtime-resource lifecycle
- [Release notes](packages/sdk/docs/release.md)

## Contribute

The SDK, CLI, contracts, conformance helpers, user-test harness, and
documentation sources are open in this repo.

- Contributor flow (fork, PR, CI expectations): [`CONTRIBUTING.md`](CONTRIBUTING.md)
- Repository layout and package commands: [`pnpm-workspace.yaml`](pnpm-workspace.yaml)
- Security disclosures: [`SECURITY.md`](SECURITY.md)

## FAQ

**Which providers are supported?**
Anthropic, DeepSeek, OpenAI, Gemini, and Mistral are accepted by the public
submission schema. Anthropic and DeepSeek are live-verified; OpenAI, Gemini,
and Mistral are accepted but live-unverified in the generated matrix. Anthropic
is the default provider; set `provider` for the others. Omit `runtime` or pass
`runtime: "managed"`; every provider uses the managed runtime and the same
`submit` shape / `RunnerEvent` stream. See the [generated capability matrix](packages/sdk/docs/provider-runtime-capabilities.md).

**How do provider keys work?**
**BYOK.** Provider keys, MCP `Authorization` headers, and any
auxiliary secrets travel inline with each submission as a single
`secrets` bundle. They live in run-scoped custody for the lifetime of the
run and aex attempts cleanup/revocation for aex-held references
at terminal. We never persist tenant provider keys as workspace-level
connections; provider-side sessions and data remain under the selected
provider account's policies.

**Is there a managed-key option?**
**Managed key — planned.** Until then it's BYOK only.

**Can I self-host or run aex in my cloud?**
No — self-hosting (running aex in your own cloud) is not supported. The
supported product boundary is the hosted aex control plane. See
the [product boundary page](packages/sdk/docs/product-boundaries.md).

## License

Apache License 2.0. See [`LICENSE`](LICENSE).
