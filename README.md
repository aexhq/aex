# aex

**Agent Executor.** aex is an agent execution platform for launching autonomous agents from a simple TypeScript SDK and CLI.

## Features

- **Agent runtime.** Managed autonomous runs with shell, filesystem, editing, web fetch/search, background commands, code execution, git, and subagents.
- **Durable infrastructure.** Run records, status, wait/cancel/delete, idempotency, typed events, output capture, downloads, timeouts, and runtime sizes.
- **Agent composition.** Skills, files, AGENTS.md, remote MCP servers, environment variables, packages, and networking controls.
- **Subagents.** Typed parent/child lineage for async child runs, output handoff, and bounded agent delegation.
- **Models and providers.** Anthropic, DeepSeek, OpenAI, Gemini, Mistral, OpenRouter, Doubao, and Doubao China behind one submission shape.
- **Typed control surface.** Strongly typed SDK inputs, CLI parity, BYOK provider keys, workspace secrets, redaction, and output modes.

## Install

```bash
npm i @aexhq/sdk
```

The package includes the TypeScript SDK and the bundled `aex` CLI used below.

aex is currently in **invite-only beta** — workspaces and API keys are issued
by the aex team (contact <support@aex.dev> for beta access). Once you have
access, create a quickstart SDK token with `runs:read`, `runs:write`,
`outputs:read`, and `billing:read` in the dashboard at <https://aex.dev>, then
set both credentials before running the examples: `AEX_API_KEY` authenticates
to aex, and `ANTHROPIC_API_KEY` is your BYOK provider key for Claude.

An API key is self-describing: the SDK constructor reads its plane from the key
and routes to it (`prd` → `https://api.aex.dev`, `dev` →
`https://dev-api.aex.dev`) with zero network. A key whose plane disagrees with
an explicit `baseUrl` throws a
`CredentialValidationError` up front, instead of a late `token_invalid`.

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
  overrides: { idleTtl: "3m" },
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

const first = await session.send("Write a short report and save it as a file.").done();
console.log(first.text);

// Later, even in another process:
const resumed = await aex.openSession(session.id);
await resumed.send("Now run the validation command and summarize the result.").done();
```

For one-shot convenience, `run()` opens a resumable session, sends one message,
and returns the collected turn:

```ts
const result = await aex.run({
  model: Models.CLAUDE_HAIKU_4_5,
  message: "Summarize this repo.",
  apiKeys: { anthropic: process.env.ANTHROPIC_API_KEY! }
});

console.log(result.runId); // session id; pass to openSession(...) to continue
console.log(result.text);
```

The bundled CLI keeps the familiar one-shot command. The `aex` binary ships
inside the package, so invoke it with `npx aex …` after a local install (or
`npm i -g @aexhq/sdk` to put a bare `aex` on your PATH):

```bash
npx aex run \
  --api-key "$AEX_API_KEY" \
  --anthropic-api-key "$ANTHROPIC_API_KEY" \
  --model claude-haiku-4-5 \
  --prompt "Write a short report and save it as a file." \
  --follow
```

## Docs

- [Quickstart](packages/sdk/docs/quickstart.md)
- [Composition](packages/sdk/docs/concepts/composition.md)
- [Secrets](packages/sdk/docs/secrets.md)
- [Limits](packages/sdk/docs/limits.md)
- [Provider/runtime capabilities](packages/sdk/docs/provider-runtime-capabilities.md)

## Contribute

The public SDK, CLI, contracts, conformance helpers, user-test harness, and docs live in this repo.

- Contributor flow: [CONTRIBUTING.md](CONTRIBUTING.md)
- Security disclosures: [SECURITY.md](SECURITY.md)
- License: [Apache License 2.0](LICENSE)
