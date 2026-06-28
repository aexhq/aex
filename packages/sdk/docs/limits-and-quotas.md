---
title: Limits & quotas
---

# Limits & quotas

These are the hard ceilings and caps that bound a run, a workspace, and a single
request. Every value is mirrored from a single source-of-truth constant; the
constant file is authoritative and this page is generated documentation, not a
second source of truth. If a value here ever disagrees with that constant,
the constant wins.

Each row is named by its source-of-truth constant. For the values that apply
when you omit an option, see
[Defaults](defaults.md). For product/policy boundaries (what aex does and does
not promise), see [Limits](limits.md).

Each row is tagged with its **source**:

- **aex policy** — an aex platform ceiling, the same for every workspace.
- **Workspace default** — a per-workspace value with a configurable override.

And whether you can **raise** it: per-run option, per-plan, or no.

## Run scope

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Maximum run timeout | 6 hours | aex policy | Per plan (billing-driven) | `RUN_MAX_TIMEOUT_MS` |
| Minimum run timeout | 1 minute | aex policy | No (floor) | `RUN_MIN_TIMEOUT_MS` |
| Per-call exec timeout (default) | 30 minutes | aex policy | Per-call via the tool call's `timeoutMs` | `RUN_DEFAULT_EXEC_TIMEOUT_MS` |
| Post-hook timeout (default) | 60 minutes | aex policy | Per-run via the hook's `timeoutMs` | `RUN_DEFAULT_POST_HOOK_TIMEOUT_MS` |
| MCP connect timeout (default) | 30 seconds | aex policy | Per-port via `connectTimeoutMs` | `RUN_DEFAULT_MCP_CONNECT_TIMEOUT_MS` |
| MCP call timeout (default) | 30 minutes | aex policy | Per-port via `callTimeoutMs` | `RUN_DEFAULT_MCP_CALL_TIMEOUT_MS` |
| Per-run spend cap | None by default; when set, the run is stopped once its spend would exceed the cap | aex policy | Per-run via `options.limits.maxSpendUsd` (a positive USD amount) | — |

### Output capture (per run)

Files stream from the container to object storage one at a time. When a cap is
reached, remaining files are **dropped and counted in the summary**, never
silently lost.

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Capture wall-clock budget | 1 hour | aex policy | No (hard ceiling) | `RUN_CAPTURE_DEFAULT_TIMEOUT_MS` |
| Max files captured | 50,000 | aex policy | No (hard ceiling) | `RUN_CAPTURE_MAX_FILES` |
| Max bytes per captured file | 1 TB | aex policy | No (hard ceiling) | `RUN_CAPTURE_MAX_FILE_BYTES` |
| Max total captured bytes | 1 TB | aex policy | No (hard ceiling) | `RUN_CAPTURE_MAX_TOTAL_BYTES` |

### Tool output caps (per run)

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| `web_fetch` returned body | 500 KB (UTF-8) | aex policy | Per-call via `max_bytes` | `REQUEST_WEB_FETCH_DEFAULT_MAX_BYTES` |
| `bash_output` per-read body | 1 MB (UTF-8) | aex policy | No (hard ceiling) | `RUN_BASH_BG_OUTPUT_MAX_BYTES` |
| `grep` max file size (larger files skipped — use `bash grep`) | 25 MB | aex policy | No (hard ceiling) | `RUN_TOOL_GREP_MAX_FILE_BYTES` |
| `head`/`tail` max file size (larger files rejected — use `bash head`/`tail`) | 100 MB | aex policy | No (hard ceiling) | `RUN_TOOL_HEAD_TAIL_MAX_FILE_BYTES` |
| `grep`/`glob` files visited per recursive walk (then truncates with a notice) | 100,000 | aex policy | No (hard ceiling) | `RUN_TOOL_WALK_MAX_FILES` |

### Subagents (per run lineage)

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Max subagent depth (public submit / `subagent` tool) | 5 (a depth-5 lineage may not spawn deeper) | aex policy | Per-run via `options.limits.maxSubagentDepth` (clamped to this hard ceiling) | `RUN_MAX_PUBLIC_SUBAGENT_DEPTH` |
| Concurrent child runs per lineage root | 1000 live (non-terminal); hard ceiling 4096 | aex policy | Per-run via `options.limits.maxConcurrentChildRuns` (clamped to the 4096 ceiling) | `RUN_DEFAULT_MAX_CONCURRENT_CHILD_RUNS` |

### Retention (per run)

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Per-run metadata KV record TTL | 24 hours | aex policy | No (hard ceiling) | `RUN_KV_RECORD_TTL_SECONDS` |
| Per-run secret-envelope KV TTL | 24 hours | aex policy | No (hard ceiling) | `RUN_KV_SECRET_TTL_SECONDS` |

## Workspace scope

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Workspace storage cap | 50 GiB (admins uncapped — not a customer entitlement) | Workspace default | Per-plane via env `AEX_WORKSPACE_STORAGE_CAP_BYTES` | `WORKSPACE_DEFAULT_STORAGE_CAP_BYTES` |
| Max concurrent runs per workspace | Advisory — there is no hard per-workspace concurrent-run cap constant; concurrency is bounded by plan, the subagent child-run cap, and provider/platform throughput rather than a fixed number. | aex policy | n/a | — |
| Skill bundle max compressed size (`.zip`) | 100 GB | Workspace default | Per-workspace (plan/env) | `WORKSPACE_SKILL_BUNDLE_MAX_COMPRESSED_BYTES` |
| Skill bundle max file entries | 1,000 | Workspace default | Per-workspace (plan/env) | `WORKSPACE_SKILL_BUNDLE_MAX_FILES` |
| Skill bundle max directory depth (`a/b/c/d` = 4) | 16 | Workspace default | Per-workspace (plan/env) | `WORKSPACE_SKILL_BUNDLE_MAX_DEPTH` |
| Skill bundle max entry path length | 512 characters | Workspace default | No (hard ceiling) | `WORKSPACE_SKILL_BUNDLE_MAX_PATH_LENGTH` |
| `File.mountPath` max length | 512 characters | Workspace default | No (hard ceiling) | `WORKSPACE_MOUNT_PATH_MAX_LENGTH` |

### Rate limits (per workspace, per minute)

Default values; each is overridable per-plane via the matching
`AEX_RATE_LIMIT_<ACTION>_PER_MINUTE` env var.

| Action | Default per minute | Source | Constant |
| --- | --- | --- | --- |
| Run submit | 60 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| Run cancel | 30 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| Run delete | 30 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| Signed output link | 120 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| API token create | 10 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| API token delete | 30 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |

## Request scope (proxy and egress)

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Proxy request body | 10 MiB | aex policy | Per-endpoint via `maxRequestBytes` | `REQUEST_PROXY_DEFAULT_MAX_REQUEST_BYTES` |
| Proxy response body | `0` = unlimited (streamed unbuffered) | aex policy | Per-endpoint via `maxResponseBytes` | `REQUEST_PROXY_DEFAULT_MAX_RESPONSE_BYTES` |
| Proxy upstream timeout | 5 minutes | aex policy | Per-endpoint via `timeoutMs` | `REQUEST_PROXY_DEFAULT_TIMEOUT_MS` |
| Signed output URL TTL | 300 seconds | aex policy | Per-call via `expiresSeconds` | `REQUEST_PRESIGN_URL_DEFAULT_TTL_SECONDS` |
| Event-stream connection ticket TTL | 60 seconds | aex policy | Per-mint via `ttlMs` | `REQUEST_TICKET_DEFAULT_TTL_MS` |
