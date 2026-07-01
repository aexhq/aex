---
title: Defaults
---

# Defaults

These are the values aex applies when you **omit** the corresponding option on a
run. Every value is mirrored from a single source-of-truth constant; the
constant file is authoritative and this page is generated documentation, not a
second source of truth. If a value here ever disagrees with that constant,
the constant wins.

Each value below is named by its source-of-truth constant. The runtime-size
presets are defined in the public
[`packages/contracts/src/runtime-sizes.ts`](https://github.com/aexhq/aex/blob/main/packages/contracts/src/runtime-sizes.ts).
For the hard ceilings and who can raise them, see
[Limits & quotas](limits-and-quotas.md). For policy boundaries, see
[Limits](limits.md).

## Run

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| `timeout` (run deadline) | 1 hour | Per-run via `options.timeout` (e.g. `"30m"`, `"2h"`), clamped to the run-timeout floor/ceiling. | `RUN_DEFAULT_TIMEOUT_MS` |
| `runtimeSize` (machine size) | `shared-0.25x-1gb` — 0.25 vCPU, 1 GB | Per-run via `options.runtimeSize` (use `RuntimeSizes.*` in TypeScript). | `RUN_DEFAULT_RUNTIME_SIZE` |
| `limits.maxSpendUsd` (per-run spend cap) | None — no per-run spend cap (the run is still bounded by its `timeout` and any workspace-level cap) | Per-run via `options.limits.maxSpendUsd` (a positive USD amount); the run is stopped once its spend would exceed the cap. | — |

## Tools

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Per-call exec timeout | 30 minutes | Per-call via the tool call's `timeoutMs`. | `RUN_DEFAULT_EXEC_TIMEOUT_MS` |
| `web_fetch` returned body | 500 KB (UTF-8) | Per-call via the tool's `max_bytes` argument. | `REQUEST_WEB_FETCH_DEFAULT_MAX_BYTES` |

## MCP

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| MCP connect timeout (register + initialize + discover) | 30 seconds | Per-port via `connectTimeoutMs`. | `RUN_DEFAULT_MCP_CONNECT_TIMEOUT_MS` |
| MCP `tools/call` timeout | 30 minutes | Per-port via `callTimeoutMs`. | `RUN_DEFAULT_MCP_CALL_TIMEOUT_MS` |

## Proxy endpoints

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| `maxRequestBytes` | 10 MiB | Per-endpoint via the endpoint's `maxRequestBytes`. | `REQUEST_PROXY_DEFAULT_MAX_REQUEST_BYTES` |
| `maxResponseBytes` | `0` (unlimited — the response is streamed unbuffered) | Per-endpoint via the endpoint's `maxResponseBytes`. | `REQUEST_PROXY_DEFAULT_MAX_RESPONSE_BYTES` |
| `timeoutMs` (upstream) | 5 minutes | Per-endpoint via the endpoint's `timeoutMs`. | `REQUEST_PROXY_DEFAULT_TIMEOUT_MS` |

## Links (signed URLs and tickets)

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Output link / signed-URL TTL | 300 seconds (5 minutes) at the storage layer; `outputLink(...)` defaults to `"1h"` | Per-call via `expiresSeconds` (storage) or `expiresIn` on `outputLink` / `fetchOutput`. | `REQUEST_PRESIGN_URL_DEFAULT_TTL_SECONDS` |
| Event-stream connection ticket TTL | 60 seconds | Per-mint via the `ttlMs` argument. | `REQUEST_TICKET_DEFAULT_TTL_MS` |

## Subagents

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Concurrent child runs per lineage root | 1000 (live, non-terminal child runs) | Per-run via `options.limits.maxConcurrentChildRuns`, clamped to the 4096 platform ceiling. | `RUN_DEFAULT_MAX_CONCURRENT_CHILD_RUNS` |
| Max subagent depth | 5 | Per-run via `options.limits.maxSubagentDepth`, clamped to the same hard ceiling. | `RUN_MAX_PUBLIC_SUBAGENT_DEPTH` |

## Workspace

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Per-workspace mutation rate limits (per minute) | run submit 60, run cancel 30, run delete 30, signed link 120, API token create 10, API token delete 30 | Per-plane via the matching `AEX_RATE_LIMIT_<ACTION>_PER_MINUTE` env var. | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| Workspace storage cap | 50 GiB | Per-plane via env `AEX_WORKSPACE_STORAGE_CAP_BYTES`; admin workspaces are uncapped (not a customer entitlement). | `WORKSPACE_DEFAULT_STORAGE_CAP_BYTES` |
