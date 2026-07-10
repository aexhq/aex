---
title: aex
---

# @aexhq/sdk

aex is an agent execution platform for launching autonomous agents from a simple TypeScript SDK and CLI.

The package ships:

- `Aex` for sessions, one-shot sessions, inspect, download, cancel, and delete.
- `sessions` / `openSession()` for durable, resumable agent sessions.
- Typed run primitives: `Models`, `Providers`, `Sizes`, `Skill`, `Tool` / `Tools`, `AgentsMd`, `File`, `McpServer`, and `Secret`.
- A bundled `aex` CLI with the same run, status, events, files, download, cancel, delete, and whoami operations.

## Install

```bash
npm i @aexhq/sdk
```

This installs the TypeScript SDK exports and the bundled `aex` CLI. The CLI
ships inside the package — invoke it with `npx aex …` after a local install, or
`npm i -g @aexhq/sdk` to put a bare `aex` on your PATH. Set both credentials
before running the examples: `AEX_API_KEY` authenticates to aex, and
`ANTHROPIC_API_KEY` is your BYOK provider key for Claude.

```bash
export AEX_API_KEY="<your-aex-api-key>"
export ANTHROPIC_API_KEY="<your-anthropic-api-key>"
```

## First Session

```ts
import { Aex, Models, Sizes } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_API_KEY!);

const session = await aex.openSession({
  model: Models.CLAUDE_HAIKU_4_5,
  system: "You are a concise engineering assistant.",
  runtime: Sizes.SHARED_0_25X_1GB,
  // Default is "3m"; set it explicitly when you want a different idle window.
  overrides: { idleTtl: "3m" },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

// done() awaits settle by default, so status is the terminal outcome
// ("succeeded"), and costUsd/usage are populated.
const first = await session.send("Summarize this repo.").done();
console.log(first.status, first.costUsd, first.text);

const resumed = await aex.openSession(session.id);
await resumed.send("Continue with the follow-up validation.").done();
```

Need a one-shot convenience? `start()` opens a session, sends `message` as one
turn, and returns the collected result. The returned `sessionId` is the session id,
so it can be resumed later with `openSession(sessionId)`.

```ts
const result = await aex.start({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! },
  message: "Write the report and save files."
});

console.log(result.sessionId, result.status, result.text);
```

For multiple providers, include each BYOK key in `apiKeys`:

```ts
await aex.start({
  model: Models.CLAUDE_HAIKU_4_5,
  apiKeys: {
    anthropic: process.env.ANTHROPIC_API_KEY!,
    openai: process.env.OPENAI_API_KEY!
  },
  message: "Delegate research to a subagent."
});
```

The same request can run from the bundled CLI (`npx aex` on a local install):

```bash
npx aex start \
  --api-key "$AEX_API_KEY" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write the report and save files." \
  --follow
```

## CLI: login, discovery, typed errors

Stop re-passing `--api-key` on every command — log in once and the token (plus
your default `--aex-url`) is persisted to a `0600` config file
(`$XDG_CONFIG_HOME/aex/config.json` or `~/.config/aex/config.json`; `%APPDATA%\aex\config.json`
on Windows). An explicit `--api-key` flag always overrides the stored one.

```bash
npx aex login --api-key "$AEX_API_KEY" [--aex-url https://api.aex.dev]
npx aex whoami            # no --api-key needed after login
npx aex whoami --json     # machine-readable workspace + scopes + limits
npx aex auth status       # show the resolved config (the token value is never printed)
npx aex logout            # clear the stored token
```

Discover the closed sets the platform accepts — no token, no network (human table
by default, machine JSON under `--json`). Per-verb `--help` is also key-free:

```bash
npx aex models list           # canonical models + their default provider
npx aex providers list        # providers + the models each serves
npx aex tools list            # complete builtin tool set
npx aex starttime-sizes list    # managed runtime presets (cpus / memory / default)
npx aex start --help            # flags for one verb (no API key required)
```

Errors are typed and actionable. Every `openSession()` / `start()` config-validation
failure throws a `SessionConfigValidationError` (`err.code === "SESSION_CONFIG_INVALID"`)
you can `catch` by code; CLI failures print a JSON envelope carrying the HTTP
`status`, a one-line `remedy`, and the `sessionId` where known, and a wrong `--model`,
`--provider`, or `--runtime-size` gets a "did you mean?" suggestion.

## Continue with sessions

Use sessions for conversational flows. `start()` is the one-shot convenience; a
`SessionHandle` is the lower-level surface when you want multiple turns,
streaming, messages, events, files, or downloads.

```ts
const session = await aex.openSession(result.sessionId);
const next = await session.send("Turn this into a checklist.").done();
console.log(next.text);

const messages = await session.messages().list();
const files = await aex.sessions.files(session.id).list();
```

## Feature Areas

- **Agent runtime:** managed autonomous sessions with filesystem read/edit, grep/glob/head/tail, open web fetch/search, background commands, code execution, git, and subagents.
- **Durable infrastructure:** session records, status, wait/cancel/delete, idempotency, typed events, file capture, downloads, timeouts, and runtime sizes.
- **Agent composition:** skills, files, AGENTS.md, remote MCP servers, environment variables, packages, and networking controls.
- **Subagents:** typed parent/child lineage for async child sessions, file handoff, and bounded agent delegation.
- **Models and providers:** Anthropic, DeepSeek, OpenAI, Gemini, Mistral, OpenRouter, Doubao, and Doubao China behind one submission shape.
- **Typed control surface:** strongly typed SDK inputs, CLI parity, BYOK provider keys, workspace secrets, redaction, and output modes.

## Docs

- [Quickstart](docs/quickstart.md)
- [SessionRecord configuration](docs/session-config.md)
- [Composition](docs/concepts/composition.md)
- [Secrets](docs/secrets.md)
- [Limits](docs/limits.md)
- [Provider/runtime capabilities](docs/provider-runtime-capabilities.md)
