# Aex

Open distributed agent cloud platform

[![npm version](https://img.shields.io/npm/v/@aexhq/sdk.svg)](https://www.npmjs.com/package/@aexhq/sdk)

> **Development status:** AEX is in active prelaunch development. Not all
> documented capabilities are deployed or verified, and the SDK, CLI, APIs,
> and hosted service are not guaranteed to work. Expect breaking changes and
> interruptions; do not rely on AEX for production workloads yet.

## Features

- Eight-hour, multi-message sessions with one retained MicroVM generation
- Direct BYOK model access with an explicitly pinned provider credential
- Automatic suspension after 180 idle seconds and automatic resume on the next
  message or live-file call
- Durable workspace files plus generation-local live files
- `read_file`, `edit_file`, `write_file`, and Bash inside the MicroVM
- Recursive subagents within one customer session
- Typed observations, telemetry exports, OpenTelemetry, and AG-UI streams

## Start

```bash
npm install @aexhq/sdk
```

## Contact
- support@aex.dev

