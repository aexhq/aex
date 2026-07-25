---
title: Defaults
---

# Defaults

These are the public values aex applies when you omit the corresponding option.
Runtime-size presets are defined in public
[`runtime-sizes.ts`](https://github.com/aexhq/aex/blob/main/packages/contracts/src/runtime-sizes.ts).
For hard ceilings and adjustable limits, see
[Limits & quotas](limits-and-quotas.md).

## Session

| Option | Default | How to override |
| --- | --- | --- |
| `timeout` | 8 hours | `overrides.timeout` (minimum 1 minute, maximum 8 hours) |
| `runtime` | `0.25cpu-1gb` (0.25 vCPU, 1 GB) | `runtime` or `Sizes.*` |
| `overrides.maxSpendUsd` | No per-session spend cap | A positive USD amount |
| `overrides.maxTurns` | 20 iterations | A positive integer, up to 200 |

## Tools and MCP

| Option | Default | How to override |
| --- | --- | --- |
| Per-call exec timeout | 30 minutes | Tool call `timeoutMs` |
| `web_fetch` returned body | 500 KB (UTF-8) | Tool argument `max_bytes` |
| MCP connect timeout | 30 seconds | MCP server `connectTimeoutMs` |
| MCP `tools/call` timeout | 30 minutes | MCP server `callTimeoutMs` |

## Links and tickets

| Option | Default | How to override |
| --- | --- | --- |
| Signed URL TTL | 300 seconds at the API layer; `session.files.link(...)` defaults to `"1h"` | `expiresSeconds` or the SDK's `expiresIn` |
| Event-stream ticket TTL | 60 seconds | `ttlMs` |

## Subagents

Subagent breadth and depth use managed budgets rather than fixed public numeric
entitlements. The service supports high recursive depth and large fan-out;
contact support before relying on unusually large workloads.

## Workspace

Workspace storage is bounded by your plan's monthly storage grant, not by a
fixed per-workspace number. The Free plan includes 5 GB; paid plans have no
per-dimension storage quota and bill usage beyond the included allowance once
you add a payment method and enable overage. Admission, concurrency, and other
adjustable workspace limits are returned by `aex.whoami()` (CLI: `aex whoami`);
contact support when the effective value does not fit your workload.
