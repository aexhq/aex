# Aex

A session-centered distributed agent runtime.

[![npm version](https://img.shields.io/npm/v/@aexhq/sdk.svg)](https://www.npmjs.com/package/@aexhq/sdk)

> **Development status:** AEX is in active prelaunch development. Not all
> documented capabilities are deployed or verified, and the SDK, CLI, APIs,
> and hosted service are not guaranteed to work. Expect breaking changes and
> interruptions; do not rely on AEX for production workloads yet.

## Features

- Multi-message sessions with direct, encrypted BYOK access to OpenAI,
  Anthropic, DeepSeek, xAI, Meta, Moonshot AI, and Alibaba model families
- One default-on sandbox prepared eagerly, suspended while idle, and resumed on
  the first waiting sandbox tool call; sessions can explicitly disable it
- Latest-only workspace files from inline bytes, HTTPS URLs, or direct upload,
  with frozen session mounts and model-invoked `storage.persist`
- Built-in file, Bash, storage, and MCP tools; large results stay in sandbox
  files unless the model explicitly invokes `storage.persist`
- Durable parallel tool effects and native subagents, bounded to 12 children
  per session lifetime and depth 3
- Live assistant previews with committed reconciliation, retained telemetry,
  replay, and compressed download
- Essential prepaid billing: saved-card display metadata, hosted setup/top-up,
  balance, transactions, and provider/model usage

## Start

```bash
npm install @aexhq/sdk
```

See the [documentation](https://aex.dev/docs) for the current API and SDK
example.

## Contact

- support@aex.dev

