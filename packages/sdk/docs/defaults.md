---
title: Defaults
---

# Defaults

These are the values aex applies when you **omit** the corresponding option on a
run. Every value is mirrored from a single source-of-truth constant in the
platform's limits module; this page is hand-maintained against those constants.
If a value here ever disagrees with the constant, the constant wins.

Each value below is named by its source-of-truth constant. The runtime-size
presets are defined in the public
[`packages/contracts/src/runtime-sizes.ts`](https://github.com/aexhq/aex/blob/main/packages/contracts/src/runtime-sizes.ts).
For the hard ceilings and who can raise them, see
[Limits & quotas](limits-and-quotas.md). For policy boundaries, see
[Limits](limits.md).

## Run

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| `timeout` (run deadline) | 8 hours (also the ceiling) | Per-session via `overrides.timeout` (e.g. `"30m"`, `"2h"`). Values outside the run-timeout floor (1 minute) / ceiling (8 hours) are rejected with `RunConfigValidationError` before submission. | `RUN_DEFAULT_TIMEOUT_MS` |
| `runtime` (machine size) | `shared-0.25x-1gb` — 0.25 vCPU, 1 GB | Per-session via `runtime` (use `Sizes.*` in TypeScript). | `RUN_DEFAULT_RUNTIME_SIZE` |
| `overrides.maxSpendUsd` (per-session spend cap) | None — no spend cap (the session is still bounded by its `timeout` and any workspace-level cap) | Per-session via `overrides.maxSpendUsd` (a positive USD amount); the session is stopped once its spend would exceed the cap. | — |

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

## Links (signed URLs and tickets)

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Output link / signed-URL TTL | 300 seconds (5 minutes) at the storage layer; `session.outputs().link(...)` defaults to `"1h"` | Per-call via `expiresSeconds` (storage) or `expiresIn` on `session.outputs().link` / `session.outputs().fetch`. | `REQUEST_PRESIGN_URL_DEFAULT_TTL_SECONDS` |
| Event-stream connection ticket TTL | 60 seconds | Per-mint via the `ttlMs` argument. | `REQUEST_TICKET_DEFAULT_TTL_MS` |

## Subagents

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Concurrent child runs per lineage root | 1000 (live, non-terminal child runs) | Platform default (subagents run in-process; no public per-run override). Hard ceiling 4096. | `RUN_DEFAULT_MAX_CONCURRENT_CHILD_RUNS` |
| Max subagent depth | 5 | Platform default (subagents run in-process; no public per-run override). | `RUN_MAX_PUBLIC_SUBAGENT_DEPTH` |

## Workspace

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Run submit rate (per minute) | 120 (`0` = disabled); past it `POST /runs` fails with `429 workspace_submit_rate_exceeded` | Per-plane via env `AEX_WORKSPACE_SUBMIT_RATE_PER_MIN`; per-workspace via support. | — |
| Max concurrent runs | 50 live root runs (hard ceiling 200) | Per-workspace override via support, clamped to the ceiling. | `WORKSPACE_DEFAULT_MAX_CONCURRENT_RUNS` |
| Monthly spend cap | $250 per UTC calendar month (`0` = unlimited) | Per-workspace override via support. | `WORKSPACE_DEFAULT_SPEND_CAP_USD` |
| Per-workspace mutation rate limits (per minute) | run cancel 30, run delete 30, signed link 120, API token create 10, API token delete 30 | Per-plane via the matching `AEX_RATE_LIMIT_<ACTION>_PER_MINUTE` env var. | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| Workspace storage cap | 500 GB (decimal) | Per-plane via env `AEX_WORKSPACE_STORAGE_CAP_BYTES`; admin workspaces are uncapped (not a customer entitlement). | `WORKSPACE_DEFAULT_STORAGE_CAP_BYTES` |
