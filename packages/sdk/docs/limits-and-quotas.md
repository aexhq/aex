---
title: Limits & quotas
---

# Limits & quotas

These public ceilings bound a session, workspace, or request. Defaults that
apply when you omit an option are also summarized in [Defaults](defaults.md).
Your workspace's effective adjustable limits are available from `aex.whoami()`
(CLI: `aex whoami`); contact support when a documented limit is adjustable but
your workspace needs a higher value.

## Session scope

| Limit | Value | Adjustable? |
| --- | --- | --- |
| Session timeout | 1 minute minimum; 8 hours maximum and default | Per session with `overrides.timeout` |
| Per-call exec timeout | 30 minutes by default | Per tool call with `timeoutMs` |
| MCP connect timeout | 30 seconds by default | Per MCP server with `connectTimeoutMs` |
| MCP call timeout | 30 minutes by default | Per MCP server with `callTimeoutMs` |
| Per-session spend cap | None by default | Per session with `overrides.maxSpendUsd` |
| Agent iterations | 20 by default; 200 maximum | Per session with `overrides.maxTurns` |

### SessionFile capture

When a capture cap is reached, remaining files are dropped and counted in the
capture summary.

| Limit | Value |
| --- | --- |
| Capture wall-clock budget | 1 hour |
| Files captured | 50,000 maximum |
| Bytes per captured file | 500 GB (decimal) maximum |
| Total captured bytes | 500 GB (decimal) maximum |

These two bound a single session's capture. They are not a workspace storage
cap — see [Workspace scope](#workspace-scope) for what actually bounds stored
bytes.

### Tool output

| Limit | Value | Adjustable? |
| --- | --- | --- |
| `web_fetch` returned body | 500 KB (UTF-8) by default | Per call with `max_bytes` |
| `bash_output` per-read body | 1 MB (UTF-8) | No |
| `grep` maximum file size | 25 MB; use `bash grep` for larger files | No |
| `head`/`tail` maximum file size | 100 MB; use the equivalent shell commands for larger files | No |
| `grep`/`glob` files visited per recursive walk | 100,000, then the result is truncated with a notice | No |

### Attached archives

An archive that exceeds any of these bounds is rejected before its contents are
available to session code; a partial archive is never exposed.

| Limit | Value |
| --- | --- |
| Compressed archive bytes | 64 MiB maximum |
| Expanded archive bytes | 128 MiB maximum |
| Materialized files and safe symlinks | 1,000 maximum per archive |
| Fidelity metadata | 8 MiB maximum |
| Planned entries across attached workspace-file archives | 10,000 maximum |

### Subagents

Subagent breadth and depth use managed budgets rather than fixed numeric
entitlements. The service supports high recursive depth and workloads ranging
to hundreds or thousands of live child agents, subject to admission at spawn
time. Contact support before relying on unusually large fan-out.

## Managed runtime

- Only `/workspace` persists across turns of the same session. Everything else
  is reset between turns and removed when the session ends.
- Declare OS and language packages with `environment.packages`. Packages
  installed ad hoc during a turn are best-effort and do not persist to the next
  turn. Python environments may enforce PEP 668; prefer declared packages or a
  virtual environment under `/workspace`.
- The selected runtime preset's `memoryMb` is the memory ceiling. See the public
  [`runtime-sizes.ts`](https://github.com/aexhq/aex/blob/main/packages/contracts/src/runtime-sizes.ts)
  definitions and [Defaults](defaults.md).
- `overrides.maxTurns` controls the agent-loop limit (20 by default, 200
  maximum).

## Workspace scope

Stored bytes are bounded by your plan's monthly storage grant, not by a fixed
per-workspace number. The Free plan includes 5 GB. An upload that cannot be
paid for is refused at admission with `quota_exhausted` (HTTP 409) and a
remedy telling you which of the two options applies: upgrade the plan, or add
a payment method and enable overage. Paid plans with overage enabled are not
refused; the usage is billed.

Use `aex.whoami()` to read the effective concurrency and submission limits
attached to the current workspace. Stable admission errors include
`quota_exhausted`, `workspace_concurrency_exceeded` and
`workspace_submit_rate_exceeded`.

| Limit | Value | Adjustable? |
| --- | --- | --- |
| Runtime asset archive | 64 MiB compressed, 128 MiB expanded, 1,000 materialized entries | No |
| Skill bundle directory depth | 16 | Contact support |
| Skill bundle entry path | 512 characters | No |
| `File.mountPath` | 512 characters | No |

`File.mountPath` names a directory, not a destination filename. The attached
file retains its source/archive filename below that directory; see
[Files](files.md).

## Request scope

| Limit | Value | Adjustable? |
| --- | --- | --- |
| Signed file URL TTL | 300 seconds at the API layer | Per call with `expiresSeconds` |
| Event-stream connection ticket TTL | 60 seconds | Per mint with `ttlMs` |
