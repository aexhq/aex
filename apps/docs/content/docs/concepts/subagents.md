---
title: Subagents
description: Delegate bounded sub-tasks to child agent sessions with the subagent tool.
icon: GitFork
---

A session can delegate bounded sub-tasks to **child agent sessions** with the built-in
`subagent` tool. Delegation is agent-driven: the model decides to fan work out,
each child is a real session with its own record, and the parent collects results
as the children finish. There is no client-side parent/child API — lineage is
session-internal.

## How the tool works

The agent calls `subagent` with a `prompt` and a `model` (both required), plus
optional `system`, `provider`, `runtimeSize`, `timeout`,
`includeBuiltinTools`, `tools` (builtin-tool names for the child), `skills`,
and `files`. The call is **always async**: on a successful spawn it returns
immediately with the child session id, and the parent keeps working while the child
sessions. When a child settles, the parent is notified in its loop, and it reads
the child's status and captured files on demand with the companion
`subagent_result` tool.

Children inherit the parent's vaulted BYOK provider keys server-side — the
child submission carries no secrets. Include a provider key for every provider
your subagents may use when you open the parent session (a parent holding no
key for the child's provider gets a clear `parent_missing_provider_key` tool
error). See [Credentials](/docs/guides/credentials/).

## Depth and breadth limits

Delegation is bounded by server-enforced lineage budgets. The runtime is
designed for broad fan-out, across hundreds and even thousands of concurrent
child agents per root, and for high levels of recursive subagent depth. Exact
operational limits are platform-managed and may change as the runtime scales.

| Limit | Runtime posture | Behavior at the limit |
| --- | --- | --- |
| Max recursive subagent depth | Supports high levels of nested delegation, bounded server-side. | A further spawn is rejected with a `depth_exceeded` tool error (the parent keeps running). |
| Concurrent children per lineage root | Designed to scale across hundreds, even thousands, of live descendants, bounded server-side. | Further spawns are refused until a child settles. |

The whole descendant subtree of one root shares a single depth and breadth
budget, enforced server-side at every level — a grandchild spawn counts against
the same root budget as a direct child. These budgets are not public per-session
dials today.

## Where children run: `in-process` vs `container`

By default a child sessions **in-process**: it executes as a sibling agent process
inside the parent's own machine, sharing the parent's CPU, memory, and
lifetime. This is the platform default shipped today.

- **No extra runtime cost.** The parent's machine is the billable unit, so
  in-process children bill **$0 of additional runtime** — fan-out is priced by
  the parent box, however many children it hosts. (Model-token spend is still
  whatever each child's provider calls cost on your BYOK key.)
- **Shared capacity.** N children share the parent's fixed CPU/memory. For
  large fan-outs, size the parent up (`runtime`) rather than assuming each
  child gets its own machine.
- **Joined lifecycle.** The parent's terminal waits for its in-process children,
  and their results are folded into the parent's per-child accounting. Platform
  recovery re-spawns in-process children exactly once if the parent's machine
  is replaced mid-session — settled children are never retry.

The escape valve is `host: "container"`: the child is dispatched to its **own
isolated machine** with its own runtime size and its own runtime billing, and
the parent does not host it. Use it when a child needs guaranteed capacity,
isolation from the parent's filesystem/CPU, or a different machine size than
the parent can share.

## Lineage and observability

Every child — in-process or container — is a first-class session record:

- The parent's transcript logs each spawn with the child's session id.
- Each child has its own status, typed event timeline, and captured files.
  The child's events and files are readable by id
  (`aex.sessions.files(id)` in the SDK, or the CLI's `aex events` /
  `aex files`). Child sessions are not served by the session read surface
  today — `aex.sessions.get(id)` / `aex sessions` / the CLI's `aex status`
  answer `not_found` for a child id.
- The child's files are handed back to the parent via `subagent_result`, and
  they remain independently downloadable after the lineage finishes.

## Bounding delegation

- Turn delegation off for a session by cherry-picking builtins without `subagent`
  (see [Agent tools](/docs/concepts/agent-tools/)) or setting `includeBuiltinTools: false`.
- A per-session spend cap (`overrides.maxSpendUsd`) bounds the parent's spend.
- The depth/breadth budgets above are platform-managed and are not settable
  per-session today.
