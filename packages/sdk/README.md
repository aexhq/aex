---
title: aex
---

# @aexhq/sdk

aex is an agent execution platform for launching autonomous agents from a simple TypeScript SDK and CLI.

The package ships:

- `AgentExecutor` for submit, run, wait, stream, inspect, download, cancel, and delete.
- Typed run primitives: `Models`, `Providers`, `Regions`, `RuntimeSizes`, `Skill`, `AgentsMd`, `File`, `McpServer`, `ProxyEndpoint`, and `Secret`.
- A bundled `aex` CLI with the same run, status, events, outputs, download, cancel, delete, whoami, and skills operations.

## Install

```bash
bun add @aexhq/sdk
```

## First Run

```ts
import { AgentExecutor, Models, Providers } from "@aexhq/sdk";

const aex = new AgentExecutor({
  apiToken: process.env.AEX_API_TOKEN!
});

const runId = await aex.submit({
  provider: Providers.ANTHROPIC,
  model: Models.CLAUDE_HAIKU_4_5,
  prompt: "Write the report and save outputs.",
  secrets: { apiKey: process.env.ANTHROPIC_API_KEY! }
});

for await (const event of aex.stream(runId)) {
  console.log(event.type);
}

const run = await aex.wait(runId);
console.log(run.status);

await aex.download(runId, { to: "./run.zip" });
```

The same request can run from the CLI:

```bash
aex run \
  --api-token "$AEX_API_TOKEN" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write the report and save outputs." \
  --follow
```

## Feature Areas

- **Agent runtime:** managed autonomous runs with filesystem read/edit, grep/glob/head/tail, open web fetch/search defaults, optional notebook tools, and post-hook repair.
- **Durable infrastructure:** run records, status, wait/cancel/delete, idempotency, typed events, output capture, downloads, timeouts, and runtime sizes.
- **Agent composition:** skills, files, AGENTS.md, remote MCP servers, proxy endpoints, environment variables, packages, and networking controls.
- **Subagents:** typed parent/child lineage for async child runs, output handoff, and bounded agent delegation.
- **Models and providers:** Anthropic, DeepSeek, OpenAI, Gemini, Mistral, OpenRouter, Doubao, and Doubao China behind one submission shape.
- **Typed control surface:** strongly typed SDK inputs, CLI parity, BYOK secrets, scoped proxy auth, redaction, and output modes.

## Docs

- [Quickstart](docs/quickstart.md)
- [Run configuration](docs/run-config.md)
- [Agent tools](docs/concepts/agent-tools.md)
- [Composition](docs/concepts/composition.md)
- [Secrets](docs/secrets.md)
- [Limits](docs/limits.md)
- [Provider/runtime capabilities](docs/provider-runtime-capabilities.md)
