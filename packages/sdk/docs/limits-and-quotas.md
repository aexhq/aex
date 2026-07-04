---
title: Limits & quotas
---

# Limits & quotas

These are the hard ceilings and caps that bound a run, a workspace, and a single
request. Every value is mirrored from a single source-of-truth constant in the
platform's limits module; this page is hand-maintained against those constants.
If a value here ever disagrees with the constant, the constant wins.

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
| Maximum run timeout | 8 hours (also the default when `timeout` is omitted) | aex policy | Per plan (billing-driven) | `RUN_MAX_TIMEOUT_MS` |
| Minimum run timeout | 1 minute | aex policy | No (floor) | `RUN_MIN_TIMEOUT_MS` |
| Per-call exec timeout (default) | 30 minutes | aex policy | Per-call via the tool call's `timeoutMs` | `RUN_DEFAULT_EXEC_TIMEOUT_MS` |
| MCP connect timeout (default) | 30 seconds | aex policy | Per-port via `connectTimeoutMs` | `RUN_DEFAULT_MCP_CONNECT_TIMEOUT_MS` |
| MCP call timeout (default) | 30 minutes | aex policy | Per-port via `callTimeoutMs` | `RUN_DEFAULT_MCP_CALL_TIMEOUT_MS` |
| Per-session spend cap | None by default; when set, the session is stopped once its spend would exceed the cap | aex policy | Per-session via `overrides.maxSpendUsd` (a positive USD amount) | — |

### Output capture (per run)

Files stream from the container to object storage one at a time. When a cap is
reached, remaining files are **dropped and counted in the summary**, never
silently lost.

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Capture wall-clock budget | 1 hour | aex policy | No (hard ceiling) | `RUN_CAPTURE_DEFAULT_TIMEOUT_MS` |
| Max files captured | 50,000 | aex policy | No (hard ceiling) | `RUN_CAPTURE_MAX_FILES` |
| Max bytes per captured file | 500 GB (decimal) | aex policy | No (hard ceiling) | `RUN_CAPTURE_MAX_FILE_BYTES` |
| Max total captured bytes | 500 GB (decimal) | aex policy | No (hard ceiling) | `RUN_CAPTURE_MAX_TOTAL_BYTES` |

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
| Max subagent depth (`subagent` tool) | 5 (a depth-5 lineage may not spawn deeper) | aex policy | No public per-run override (subagents run in-process) | `RUN_MAX_PUBLIC_SUBAGENT_DEPTH` |
| Concurrent child runs per lineage root | 1000 live (non-terminal); hard ceiling 4096 | aex policy | No public per-run override (subagents run in-process) | `RUN_DEFAULT_MAX_CONCURRENT_CHILD_RUNS` |

### Retention (per run)

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Per-run metadata record TTL | 24 hours | aex policy | No (hard ceiling) | `RUN_KV_RECORD_TTL_SECONDS` |
| Per-run secret-envelope TTL | 24 hours | aex policy | No (hard ceiling) | `RUN_KV_SECRET_TTL_SECONDS` |

## Workspace scope

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Workspace storage cap | 500 GB (decimal; admins uncapped — not a customer entitlement) | Workspace default | Per-plane via env `AEX_WORKSPACE_STORAGE_CAP_BYTES` | `WORKSPACE_DEFAULT_STORAGE_CAP_BYTES` |
| Max concurrent runs per workspace | Plan-based: **5** live (non-terminal) root runs on the free plan, **50** on Pro, **200** on Team; hard platform ceiling **200**. One more submit past the cap fails with `429 workspace_concurrency_exceeded` (see [Errors](errors.md)). Subagent children are governed separately by the per-lineage caps below. Read your effective cap from `aex.whoami().limits.maxConcurrentRuns`. | Workspace default (per plan) | Per-plan (upgrade) or per-workspace override (contact support), clamped to the 200 ceiling | `PLANS[planKey].maxConcurrentRuns` / `WORKSPACE_MAX_CONCURRENT_RUNS_CEILING` |
| Monthly workspace spend cap | **$250** per rolling UTC calendar month by default; `0` = unlimited. A submit past the cap fails with `402 workspace_spend_cap_exceeded` (see [Errors](errors.md)). | Workspace default | Per-workspace override (contact support) | `WORKSPACE_DEFAULT_SPEND_CAP_USD` |
| Skill bundle max compressed size (`.zip`) | 10 GiB (enforced at upload by the SDK and re-enforced server-side) | aex policy | No (hard ceiling) | `SKILL_BUNDLE_LIMITS.maxCompressedBytes` |
| Skill bundle max decompressed size (sum of uncompressed file sizes) | 50 MB | aex policy | No (hard ceiling) | `SKILL_BUNDLE_LIMITS.maxDecompressedBytes` |
| Skill bundle max file entries | 1,000 | Workspace default | Per-workspace (plan/env) | `WORKSPACE_SKILL_BUNDLE_MAX_FILES` |
| Skill bundle max directory depth (`a/b/c/d` = 4) | 16 | Workspace default | Per-workspace (plan/env) | `WORKSPACE_SKILL_BUNDLE_MAX_DEPTH` |
| Skill bundle max entry path length | 512 characters | Workspace default | No (hard ceiling) | `WORKSPACE_SKILL_BUNDLE_MAX_PATH_LENGTH` |
| `File.mountPath` max length | 512 characters | Workspace default | No (hard ceiling) | `WORKSPACE_MOUNT_PATH_MAX_LENGTH` |

### Rate limits (per workspace, per minute)

Run submission has its own platform-enforced velocity cap: **120 submits per
minute** per workspace by default (`0` = disabled). Past it, `POST /runs` fails
with `429 workspace_submit_rate_exceeded` (see [Errors](errors.md)). It is
overridable per-plane via `AEX_WORKSPACE_SUBMIT_RATE_PER_MIN` or per-workspace
via support.

The dashboard mutation actions below default as listed; each is overridable
per-plane via the matching `AEX_RATE_LIMIT_<ACTION>_PER_MINUTE` env var.

| Action | Default per minute | Source | Constant |
| --- | --- | --- | --- |
| Run cancel | 30 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| Run delete | 30 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| Signed output link | 120 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| API token create | 10 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |
| API token delete | 30 | Workspace default | `WORKSPACE_RATE_LIMIT_DEFAULTS` |

### Introspecting your effective caps

`aex.whoami()` (CLI: `aex whoami`) returns a `limits` object carrying the
workspace's *effective* values for the caps above — `maxConcurrentRuns`,
`submitRatePerMinute`, `spendCapUsd`, plus the live `monthSpendUsd`,
`balanceUsd`, `balanceGraceFloorUsd`, and `paymentMethodStatus` — resolved by
the same code the admission gates use, so you can anticipate a `429`/`402`
before submitting. See [Authentication](authentication.md) and
[Errors](errors.md).

## Request Scope

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Signed output URL TTL | 300 seconds | aex policy | Per-call via `expiresSeconds` | `REQUEST_PRESIGN_URL_DEFAULT_TTL_SECONDS` |
| Event-stream connection ticket TTL | 60 seconds | aex policy | Per-mint via `ttlMs` | `REQUEST_TICKET_DEFAULT_TTL_MS` |
