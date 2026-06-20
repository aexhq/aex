---
title: Limits & quotas
---

# Limits & quotas

These are the hard ceilings and caps that bound a run, a workspace, and a single
request. Every value is mirrored from a single source-of-truth constant; the
constant file is authoritative and this page is generated documentation, not a
second source of truth. If a value here ever disagrees with the linked constant,
the constant wins.

Source of truth:
[`packages/shared/src/limits.ts`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts).
For the values that apply when you omit an option, see
[Defaults](defaults.md). For product/policy boundaries (what aex does and does
not promise), see [Limits](limits.md).

Each row is tagged with its **source**:

- **aex policy** — an aex platform ceiling, the same for every workspace.
- **Workspace default** — a per-workspace value with a configurable override.

And whether you can **raise** it: per-run option, per-plan, or no.

## Run scope

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Maximum run timeout | 6 hours | aex policy | Per plan (billing-driven) | [`RUN_MAX_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L163) |
| Minimum run timeout | 1 minute | aex policy | No (floor) | [`RUN_MIN_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L172) |
| Per-call exec timeout (default) | 30 minutes | aex policy | Per-call via the tool call's `timeoutMs` | [`RUN_DEFAULT_EXEC_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L202) |
| Post-hook timeout (default) | 60 minutes | aex policy | Per-run via the hook's `timeoutMs` | [`RUN_DEFAULT_POST_HOOK_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L234) |
| MCP connect timeout (default) | 30 seconds | aex policy | Per-port via `connectTimeoutMs` | [`RUN_DEFAULT_MCP_CONNECT_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L255) |
| MCP call timeout (default) | 30 minutes | aex policy | Per-port via `callTimeoutMs` | [`RUN_DEFAULT_MCP_CALL_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L265) |

### Output capture (per run)

Files stream from the container to object storage one at a time. When a cap is
reached, remaining files are **dropped and counted in the summary**, never
silently lost.

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Capture wall-clock budget | 1 hour | aex policy | No (hard ceiling) | [`RUN_CAPTURE_DEFAULT_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L288) |
| Max files captured | 50,000 | aex policy | No (hard ceiling) | [`RUN_CAPTURE_MAX_FILES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L297) |
| Max bytes per captured file | 1 TB | aex policy | No (hard ceiling) | [`RUN_CAPTURE_MAX_FILE_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L307) |
| Max total captured bytes | 1 TB | aex policy | No (hard ceiling) | [`RUN_CAPTURE_MAX_TOTAL_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L315) |

### Tool output caps (per run)

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| `web_fetch` returned body | 500 KB (UTF-8) | aex policy | Per-call via `max_bytes` | [`REQUEST_WEB_FETCH_DEFAULT_MAX_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L528) |
| `bash_output` per-read body | 1 MB (UTF-8) | aex policy | No (hard ceiling) | [`RUN_BASH_BG_OUTPUT_MAX_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L327) |
| `grep` max file size (larger files skipped — use `bash grep`) | 25 MB | aex policy | No (hard ceiling) | [`RUN_TOOL_GREP_MAX_FILE_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L341) |
| `head`/`tail` max file size (larger files rejected — use `bash head`/`tail`) | 100 MB | aex policy | No (hard ceiling) | [`RUN_TOOL_HEAD_TAIL_MAX_FILE_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L352) |
| `grep`/`glob` files visited per recursive walk (then truncates with a notice) | 100,000 | aex policy | No (hard ceiling) | [`RUN_TOOL_WALK_MAX_FILES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L364) |

### Subagents (per run lineage)

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Max subagent depth (public submit / `subagent` tool) | 1 (a root may spawn one level of child; that child may not spawn) | aex policy | No (hard ceiling) | [`RUN_MAX_PUBLIC_SUBAGENT_DEPTH`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L417) |
| Concurrent child runs per lineage root | 4 | aex policy | Per-plane via env `AEX_MAX_CONCURRENT_CHILD_RUNS` | [`RUN_DEFAULT_MAX_CONCURRENT_CHILD_RUNS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L427) |

### Retention (per run)

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Per-run metadata KV record TTL | 24 hours | aex policy | No (hard ceiling) | [`RUN_KV_RECORD_TTL_SECONDS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L462) |
| Per-run secret-envelope KV TTL | 24 hours | aex policy | No (hard ceiling) | [`RUN_KV_SECRET_TTL_SECONDS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L474) |

## Workspace scope

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Workspace storage cap | 50 GiB (admins uncapped — not a customer entitlement) | Workspace default | Per-plane via env `AEX_WORKSPACE_STORAGE_CAP_BYTES` | [`WORKSPACE_DEFAULT_STORAGE_CAP_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L52) |
| Max concurrent runs per workspace | Advisory — there is no hard per-workspace concurrent-run cap constant; concurrency is bounded by plan, the subagent child-run cap, and provider/platform throughput rather than a fixed number. | aex policy | n/a | — |
| Skill bundle max compressed size (`.zip`) | 100 GB | Workspace default | Per-workspace (plan/env) | [`WORKSPACE_SKILL_BUNDLE_MAX_COMPRESSED_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L63) |
| Skill bundle max file entries | 1,000 | Workspace default | Per-workspace (plan/env) | [`WORKSPACE_SKILL_BUNDLE_MAX_FILES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L72) |
| Skill bundle max directory depth (`a/b/c/d` = 4) | 16 | Workspace default | Per-workspace (plan/env) | [`WORKSPACE_SKILL_BUNDLE_MAX_DEPTH`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L81) |
| Skill bundle max entry path length | 512 characters | Workspace default | No (hard ceiling) | [`WORKSPACE_SKILL_BUNDLE_MAX_PATH_LENGTH`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L89) |
| `File.mountPath` max length | 512 characters | Workspace default | No (hard ceiling) | [`WORKSPACE_MOUNT_PATH_MAX_LENGTH`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L99) |

### Rate limits (per workspace, per minute)

Default values; each is overridable per-plane via the matching
`AEX_RATE_LIMIT_<ACTION>_PER_MINUTE` env var.

| Action | Default per minute | Source | Constant |
| --- | --- | --- | --- |
| Run submit | 60 | Workspace default | [`WORKSPACE_RATE_LIMIT_DEFAULTS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L112) |
| Run cancel | 30 | Workspace default | [`WORKSPACE_RATE_LIMIT_DEFAULTS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L112) |
| Run delete | 30 | Workspace default | [`WORKSPACE_RATE_LIMIT_DEFAULTS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L112) |
| Signed output link | 120 | Workspace default | [`WORKSPACE_RATE_LIMIT_DEFAULTS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L112) |
| API token create | 10 | Workspace default | [`WORKSPACE_RATE_LIMIT_DEFAULTS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L112) |
| API token delete | 30 | Workspace default | [`WORKSPACE_RATE_LIMIT_DEFAULTS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L112) |

## Request scope (proxy and egress)

| Limit | Value | Source | Raisable? | Constant |
| --- | --- | --- | --- | --- |
| Proxy request body | 10 MiB | aex policy | Per-endpoint via `maxRequestBytes` | [`REQUEST_PROXY_DEFAULT_MAX_REQUEST_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L499) |
| Proxy response body | `0` = unlimited (streamed unbuffered) | aex policy | Per-endpoint via `maxResponseBytes` | [`REQUEST_PROXY_DEFAULT_MAX_RESPONSE_BYTES`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L509) |
| Proxy upstream timeout | 5 minutes | aex policy | Per-endpoint via `timeoutMs` | [`REQUEST_PROXY_DEFAULT_TIMEOUT_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L519) |
| Signed output URL TTL | 300 seconds | aex policy | Per-call via `expiresSeconds` | [`REQUEST_PRESIGN_URL_DEFAULT_TTL_SECONDS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L539) |
| Event-stream connection ticket TTL | 60 seconds | aex policy | Per-mint via `ttlMs` | [`REQUEST_TICKET_DEFAULT_TTL_MS`](https://github.com/aexhq/aex-platform/blob/main/packages/shared/src/limits.ts#L549) |
