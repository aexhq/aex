# Aex

High performance, scalable, distributed agent runtime.

[![npm version](https://img.shields.io/npm/v/@aexhq/sdk.svg)](https://www.npmjs.com/package/@aexhq/sdk)

> **Development status:** AEX is in active prelaunch development. Not all
> documented capabilities are deployed or verified, and the SDK, CLI, APIs,
> and hosted service are not guaranteed to work. Expect breaking changes and
> interruptions; do not rely on AEX for production workloads yet.

## Features
- Supported models: 6 native models, vercel ai gateway and openrouter
- Zero token markup with byok
- Automatic suspend & resume, pay only for storage on idle between turns
- Subagents scale horizontally, easily over hundreds, support recursive spawn
- Tools: built-in bash, todo, web_fetch/web_search, custom tools & files etc
- Fast milliseconds start-up via firecracker microvm
- Skills & MCP Server support
- Full observability, Open Telemetry & AG-UI compatible across span, logs, traces, and events

## Start

```bash
npm install @aexhq/sdk
```

## Contact
- support@aex.dev

