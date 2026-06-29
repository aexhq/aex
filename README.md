# aex

**Agent Executor.** aex is an agent execution platform for launching autonomous agents from a simple TypeScript SDK and CLI.

## Features

- **Agent runtime.** Managed autonomous runs with shell, filesystem, editing, notebook, web fetch/search, background command, and post-hook repair tools.
- **Durable infrastructure.** Run records, status, wait/cancel/delete, idempotency, typed events, output capture, downloads, timeouts, and runtime sizes.
- **Agent composition.** Skills, files, AGENTS.md, remote MCP servers, proxy endpoints, environment variables, packages, and networking controls.
- **Subagents.** Typed parent/child lineage for async child runs, output handoff, and bounded agent delegation.
- **Models and providers.** Anthropic, DeepSeek, OpenAI, Gemini, Mistral, OpenRouter, Doubao, and Doubao China behind one submission shape.
- **Typed control surface.** Strongly typed SDK inputs, CLI parity, BYOK secrets, scoped proxy auth, redaction, and output modes.

## Install

```bash
bun add @aexhq/sdk
```

The package includes the TypeScript SDK and the bundled `aex` CLI used below.
Set both credentials before running the examples: `AEX_API_TOKEN` authenticates
to aex, and `ANTHROPIC_API_KEY` is your BYOK provider key for Claude.

```bash
export AEX_API_TOKEN="<your-aex-token>"
export ANTHROPIC_API_KEY="<your-anthropic-api-key>"
```

## First Run

```ts
import { AgentExecutor, Models } from "@aexhq/sdk";

const aex = new AgentExecutor({ apiToken: process.env.AEX_API_TOKEN! });

const runId = await aex.submit({
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "Write the report and save outputs.",
  secrets: { apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! } }
});

for await (const event of aex.stream(runId)) console.log(event.type);

await aex.wait(runId);
await aex.download(runId, { to: "./run.zip" });
```

Same shape from the bundled CLI:

```bash
aex run \
  --api-token "$AEX_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write the report and save outputs." \
  --follow
```

## Docs

- [Quickstart](packages/sdk/docs/quickstart.md)
- [Agent tools](packages/sdk/docs/concepts/agent-tools.md)
- [Composition](packages/sdk/docs/concepts/composition.md)
- [Secrets](packages/sdk/docs/secrets.md)
- [Limits](packages/sdk/docs/limits.md)
- [Provider/runtime capabilities](packages/sdk/docs/provider-runtime-capabilities.md)

## Contribute

The public SDK, CLI, contracts, conformance helpers, user-test harness, and docs live in this repo.

- Contributor flow: [CONTRIBUTING.md](CONTRIBUTING.md)
- Security disclosures: [SECURITY.md](SECURITY.md)
- License: [Apache License 2.0](LICENSE)
