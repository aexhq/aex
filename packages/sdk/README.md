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
import { AgentExecutor, Models } from "@aexhq/sdk";

const aex = new AgentExecutor({ apiToken: process.env.AEX_API_TOKEN! });

// run() submits, waits for the run to settle, and returns the result —
// no manual poll loop. `provider` is derived from the model.
const { text, ok } = await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKey: process.env.ANTHROPIC_API_KEY!,
  prompt: "Summarize this repo."
});

console.log(ok, text);
```

Need the run id, live events, or downloads? Use `submit` + `stream` + `wait`:

```ts
const runId = await aex.submit({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKey: process.env.ANTHROPIC_API_KEY!,
  prompt: "Write the report and save outputs."
});

for await (const event of aex.stream(runId)) {
  console.log(event.type);
}

const run = await aex.wait(runId);
console.log(run.status);

await aex.download(runId, { to: "./run.zip" });
```

For multiple providers (e.g. subagents on a different model family), pass a
`credentials` map instead of `apiKey`:

```ts
await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  credentials: { anthropic: process.env.ANTHROPIC_API_KEY!, openai: process.env.OPENAI_API_KEY! },
  prompt: "Delegate research to a subagent."
});
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

## CLI: login, discovery, typed errors

Stop re-passing `--api-token` on every command — log in once and the token (plus
your default `--aex-url`) is persisted to a `0600` config file
(`$XDG_CONFIG_HOME/aex/config.json` or `~/.config/aex/config.json`; `%APPDATA%\aex\config.json`
on Windows). An explicit `--api-token` flag always overrides the stored one.

```bash
aex login --api-token "$AEX_API_TOKEN" [--aex-url https://api.aex.dev]
aex whoami            # no --api-token needed after login
aex auth status       # show the resolved config (the token value is never printed)
aex logout            # clear the stored token
```

Discover the closed sets the platform accepts — no token, no network (human table
by default, machine JSON under `--json`):

```bash
aex models list           # canonical models + their default provider
aex providers list        # providers + the models each serves
aex tools list            # builtin tools (default vs opt-in, e.g. notebook_edit)
aex runtime-sizes list    # managed runtime presets (cpus / memory / default)
```

Errors are typed and actionable. Every `submit()` config-validation failure throws
a `RunConfigValidationError` (`err.code === "RUN_CONFIG_INVALID"`) you can `catch`
by code; CLI failures print a JSON envelope carrying the HTTP `status`, a one-line
`remedy`, and the `runId` where known, and a wrong `--model`/`--provider`/
`--runtime-size`/`--region` gets a "did you mean?" suggestion.

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
