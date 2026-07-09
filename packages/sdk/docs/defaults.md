---
title: Defaults
---

# Defaults

These are the values aex applies when you **omit** the corresponding option on a
session. Every value is mirrored from a single source-of-truth constant in the
platform's limits module; this page is hand-maintained against those constants.
If a value here ever disagrees with the constant, the constant wins.

Each value below is named by its source-of-truth constant. The runtime-size
presets are defined in the public
[`packages/contracts/src/runtime-sizes.ts`](https://github.com/aexhq/aex/blob/main/packages/contracts/src/runtime-sizes.ts).
For the hard ceilings and who can raise them, see
[Limits & quotas](limits-and-quotas.md). For policy boundaries, see
[Limits](limits.md).

## SessionRecord

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| `timeout` (session deadline) | 8 hours (also the ceiling) | Per-session via `overrides.timeout` (e.g. `"30m"`, `"2h"`). Values outside the session-timeout floor (1 minute) / ceiling (8 hours) are rejected with `SessionConfigValidationError` before submission. | `SESSION_DEFAULT_TIMEOUT_MS` |
| `runtime` (machine size) | `shared-0.25x-1gb` — 0.25 vCPU, 1 GB | Per-session via `runtime` (use `Sizes.*` in TypeScript). | `SESSION_DEFAULT_RUNTIME_SIZE` |
| `overrides.maxSpendUsd` (per-session spend cap) | None — no spend cap (the session is still bounded by its `timeout` and any workspace-level cap) | Per-session via `overrides.maxSpendUsd` (a positive USD amount); the session is stopped once its spend would exceed the cap. | — |
| `overrides.maxTurns` (agent iterations per session) | 20 (ceiling 200) | Per-session via `overrides.maxTurns` (a positive integer, clamped to the ceiling). | `SESSION_DEFAULT_MAX_TURNS` |

## Tools

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Per-call exec timeout | 30 minutes | Per-call via the tool call's `timeoutMs`. | `SESSION_DEFAULT_EXEC_TIMEOUT_MS` |
| `web_fetch` returned body | 500 KB (UTF-8) | Per-call via the tool's `max_bytes` argument. | `REQUEST_WEB_FETCH_DEFAULT_MAX_BYTES` |

## MCP

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| MCP connect timeout (register + initialize + discover) | 30 seconds | Per-port via `connectTimeoutMs`. | `SESSION_DEFAULT_MCP_CONNECT_TIMEOUT_MS` |
| MCP `tools/call` timeout | 30 minutes | Per-port via `callTimeoutMs`. | `SESSION_DEFAULT_MCP_CALL_TIMEOUT_MS` |

## Links (signed URLs and tickets)

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Output link / signed-URL TTL | 300 seconds (5 minutes) at the storage layer; `session.outputs().link(...)` defaults to `"1h"` | Per-call via `expiresSeconds` (storage) or `expiresIn` on `session.outputs().link` / `session.outputs().fetch`. | `REQUEST_PRESIGN_URL_DEFAULT_TTL_SECONDS` |
| Event-stream connection ticket TTL | 60 seconds | Per-mint via the `ttlMs` argument. | `REQUEST_TICKET_DEFAULT_TTL_MS` |

## Subagents

Subagent breadth and depth are platform-managed budgets rather than fixed public
numeric entitlements; exact operational values may change as capacity evolves.

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| Concurrent child sessions per lineage root | Platform-managed; designed to scale across hundreds, even thousands, of live child agents. | No public per-session override; contact support for unusually large workloads. | `SESSION_DEFAULT_MAX_CONCURRENT_CHILD_SESSIONS` |
| Max subagent depth | Platform-managed; supports high recursive subagent depth. | No public per-session override. | `SESSION_MAX_PUBLIC_SUBAGENT_DEPTH` |

## Workspace

| Option | Default | How to override | Source |
| --- | --- | --- | --- |
| SessionRecord submit rate (per minute) | 120 (`0` = disabled); past it `POST /sessions` fails with `429 workspace_submit_rate_exceeded` | Per-plane via env `AEX_WORKSPACE_SUBMIT_RATE_PER_MIN`; per-workspace via support. | — |
| Max concurrent sessions | Plan-based: 5 live root sessions (free), 50 (Pro), 200 (Team); hard ceiling 200 | Per-plan (upgrade) or per-workspace override via support, clamped to the ceiling. | `PLANS[planKey].maxConcurrentSessions` |
| Monthly spend cap | $250 per UTC calendar month (`0` = unlimited) | Per-workspace override via support. | `WORKSPACE_DEFAULT_SPEND_CAP_USD` |
| Per-workspace mutation rate limits (per minute) | session cancel 30, session delete 30, signed link 120, API key create 10, API key delete 30 | Per-plane via the matching `AEX_RATE_LIMIT_<ACTION>_PER_MINUTE` env var. | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| Workspace storage cap | 500 GB (decimal) | Per-plane via env `AEX_WORKSPACE_STORAGE_CAP_BYTES`; admin workspaces are uncapped (not a customer entitlement). | `WORKSPACE_DEFAULT_STORAGE_CAP_BYTES` |
