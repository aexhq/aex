---
title: Defaults
---

# Defaults

These are the values aex applies when you **omit** the corresponding option on a
run. Every value is mirrored from a single source-of-truth constant; the
constant file is authoritative and this page is generated documentation, not a
second source of truth. If a value here ever disagrees with the linked constant,
the constant wins.

Source of truth:
[`packages/shared/src/limits.ts`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts)
(runtime-size presets:
[`packages/contracts/src/runtime-sizes.ts`](https://github.com/aexhq/aex/blob/main/packages/contracts/src/runtime-sizes.ts)).
For the hard ceilings and who can raise them, see
[Limits & quotas](limits-and-quotas.md). For policy boundaries, see
[Limits](limits.md).

## Run

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| `timeout` (run deadline) | 1 hour | Per-run via `options.timeout` (e.g. `"30m"`, `"2h"`), clamped to the run-timeout floor/ceiling. | [`RUN_DEFAULT_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L154) |
| `runtimeSize` (machine size) | `shared-1x-128mb` — 1 vCPU, 128 MB | Per-run via `options.runtimeSize` (use `RuntimeSizes.*` in TypeScript). | [`RUN_DEFAULT_RUNTIME_SIZE`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L143) |
| `postHook.timeout` | 60 minutes | Per-run via the hook's `timeoutMs`. | [`RUN_DEFAULT_POST_HOOK_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L234) |
| `postHook.maxTurns` (repair budget) | 10 turns | Per-run via the hook's `maxTurns`. | [`RUN_DEFAULT_POST_HOOK_MAX_TURNS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L244) |

## Tools

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Per-call exec timeout | 30 minutes | Per-call via the tool call's `timeoutMs`. | [`RUN_DEFAULT_EXEC_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L202) |
| `web_fetch` returned body | 500 KB (UTF-8) | Per-call via the tool's `max_bytes` argument. | [`REQUEST_WEB_FETCH_DEFAULT_MAX_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L528) |

## MCP

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| MCP connect timeout (register + initialize + discover) | 30 seconds | Per-port via `connectTimeoutMs`. | [`RUN_DEFAULT_MCP_CONNECT_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L255) |
| MCP `tools/call` timeout | 30 minutes | Per-port via `callTimeoutMs`. | [`RUN_DEFAULT_MCP_CALL_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L265) |

## Proxy endpoints

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| `maxRequestBytes` | 10 MiB | Per-endpoint via the endpoint's `maxRequestBytes`. | [`REQUEST_PROXY_DEFAULT_MAX_REQUEST_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L499) |
| `maxResponseBytes` | `0` (unlimited — the response is streamed unbuffered) | Per-endpoint via the endpoint's `maxResponseBytes`. | [`REQUEST_PROXY_DEFAULT_MAX_RESPONSE_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L509) |
| `timeoutMs` (upstream) | 5 minutes | Per-endpoint via the endpoint's `timeoutMs`. | [`REQUEST_PROXY_DEFAULT_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L519) |

## Links (signed URLs and tickets)

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Output link / signed-URL TTL | 300 seconds (5 minutes) at the storage layer; `outputLink(...)` defaults to `"1h"` | Per-call via `expiresSeconds` (storage) or `expiresIn` on `outputLink` / `fetchOutput`. | [`REQUEST_PRESIGN_URL_DEFAULT_TTL_SECONDS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L539) |
| Event-stream connection ticket TTL | 60 seconds | Per-mint via the `ttlMs` argument. | [`REQUEST_TICKET_DEFAULT_TTL_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L549) |

## Subagents

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Concurrent child runs per lineage root | 4 | Per-plane via env `AEX_MAX_CONCURRENT_CHILD_RUNS`; not a per-run option. | [`RUN_DEFAULT_MAX_CONCURRENT_CHILD_RUNS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L427) |

## Workspace

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Per-workspace mutation rate limits (per minute) | run submit 60, run cancel 30, run delete 30, signed link 120, API token create 10, API token delete 30 | Per-plane via the matching `AEX_RATE_LIMIT_<ACTION>_PER_MINUTE` env var. | [`WORKSPACE_RATE_LIMIT_DEFAULTS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L112) |
| Workspace storage cap | 50 GiB | Per-plane via env `AEX_WORKSPACE_STORAGE_CAP_BYTES`; admin workspaces are uncapped (not a customer entitlement). | [`WORKSPACE_DEFAULT_STORAGE_CAP_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L52) |
