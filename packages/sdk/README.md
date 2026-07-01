---
title: aex
---

# @aexhq/sdk

aex is an agent execution platform for launching autonomous agents from a simple TypeScript SDK and CLI.

The package ships:

- `Aex` / `AgentExecutor` for sessions, one-shot runs, inspect, download, cancel, and delete.
- `sessions` / `openSession()` for durable, resumable agent sessions.
- Typed run primitives: `Models`, `Providers`, `RuntimeSizes`, `Skill`, `AgentsMd`, `File`, `McpServer`, `ProxyEndpoint`, and `Secret`.
- A bundled `aex` CLI with the same run, status, events, outputs, download, cancel, delete, whoami, and skills operations.

## Install

```bash
bun add @aexhq/sdk
```

This installs the TypeScript SDK exports and the bundled `aex` CLI. Set both
credentials before running the examples: `AEX_API_TOKEN` authenticates to aex,
and `ANTHROPIC_API_KEY` is your BYOK provider key for Claude.

```bash
export AEX_API_TOKEN="<your-aex-token>"
export ANTHROPIC_API_KEY="<your-anthropic-api-key>"
```

## First Session

```ts
import { Aex, Models, Sizes } from "@aexhq/sdk";

const aex = new Aex({ apiToken: process.env.AEX_API_TOKEN! });

const session = await aex.openSession({
  model: Models.CLAUDE_HAIKU_4_5,
  system: "You are a concise engineering assistant.",
  runtime: Sizes.SHARED_0_25X_1GB,
  // Default is "3m"; set it explicitly when you want a different idle window.
  overrides: { idleTtl: "3m" },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

const first = await session.send("Summarize this repo.").done();
console.log(first.status, first.text);

const resumed = await aex.openSession(session.id);
await resumed.send("Continue with the follow-up validation.").done();
```

Need a one-shot convenience? `run()` opens a session, sends `message` as one
turn, and returns the collected result. The returned `runId` is the session id,
so it can be resumed later with `openSession(runId)`.

```ts
const result = await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! },
  message: "Write the report and save outputs."
});

console.log(result.runId, result.status, result.text);
```

For multiple providers, include each BYOK key in `apiKeys`:

```ts
await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKeys: {
    anthropic: process.env.ANTHROPIC_API_KEY!,
    openai: process.env.OPENAI_API_KEY!
  },
  message: "Delegate research to a subagent."
});
```

The same request can run from the bundled CLI:

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
`remedy`, and the `runId` where known, and a wrong `--model`, `--provider`, or
`--runtime-size` gets a "did you mean?" suggestion.

## Chat over a corpus of runs

Turn a selected set of runs into a read-only chat. `createCorpusTools(client, { runIds })`
returns vendor-neutral, corpus-scoped read tools (`list_runs` / `get_run` /
`list_outputs` / `read_output` / `search_outputs`) — every tool refuses a run
outside the corpus. Drive them with any LLM; `examples/chat-corpus.ts` shows the
direct-Claude loop (`@anthropic-ai/sdk`), and the CLI ships it as a one-shot
command (BYOK; the importable SDK stays LLM-vendor-free):

```bash
aex chat --run run_<A> --run run_<B> \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-opus-4-8 \
  --prompt "Across these runs, which produced a report.md and what's its headline finding?" \
  --api-token "$AEX_API_TOKEN"
```

`AgentExecutor.searchOutputs({ runIds, filename, extension, contentType, limit })`
finds output files across runs and returns references (no bytes) you then
`readOutputText`.

## Feature Areas

- **Agent runtime:** managed autonomous runs with filesystem read/edit, grep/glob/head/tail, open web fetch/search defaults, and optional notebook tools.
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
