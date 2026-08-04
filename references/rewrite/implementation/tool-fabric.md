---
title: Tool fabric implementation plan
description: Test-first implementation plan for typed Brain-control, network, MCP, Hands, and detached tool execution.
keywords:
  - tools
  - mcp
  - hands
  - performance
audience: implementation agents and maintainers
status: proposed
related:
  - references/rewrite/README.md
  - references/rewrite/architecture-performance-v1.md
---

# Tool fabric implementation plan

Source design: `architecture-performance-v1.md`. This plan is implementation
owned by the public `aex/` repository and preserves Brain's durable effect,
lease, fence, and receipt authority.

## Ownership slices

### Brain-control typed tools

Likely ownership:

- `crates/aex-brain-application/src/activation/`
- Brain effect/domain types and DynamoDB store adapters
- `runtimes/brain-mux/src/` composition and model-request construction
- `crates/aex-brain-tool-catalog`

Add an explicit executor for a small signed catalog of todo, operation-status,
approval, and bounded session-state operations. The executor must use normal
prepared/claimed/started/settled effect transitions and conditional DDB writes.
It must reject shell, filesystem, network, and unbounded payload requests.

### Provider and managed web

Likely ownership:

- `crates/aex-brain-provider-gateway`
- `crates/aex-brain-managed-web`
- provider proof/transport adapters

Preserve the existing per-workspace/credential provider client isolation. Add
semantic stream deltas and a bounded preview coalescer. Managed web may reuse a
connection only under the exact screened origin/address/policy key; every
request and redirect still resolves and screens addresses. Disable cross-origin
HTTP/2 coalescing.

### MCP

Likely ownership:

- `crates/aex-brain-mcp`
- `crates/aex-brain-tool-catalog`
- `runtimes/brain-mux/src/` production composition

Implement the live Streamable HTTP adapter and pool sessions by organization,
workspace, server identity, secret generation, protocol revision, and manifest
digest. Freeze manifests at registration revision. MCP Task-capable calls use a
durable task identity and polling; a dropped non-task mutation becomes
ambiguous/unknown rather than being replayed.

### Hands/MicroVM

Likely ownership:

- `crates/aex-brain-hands`
- `runtimes/hands-agent`
- `runtimes/hands-image`
- runtime-control worker and MicroVM provider integration

Bash, `grep`, arbitrary scripts, package installation, and workspace/file
mutations remain inside ARM64 MicroVM generations. Do not add a host-side grep
fast path. Add generation/fence/guest/token endpoint leases, lifecycle
single-flight, durable requested->launching->running materialization, and
checksummed result receipts. Park the Brain activation while launch/resume is
pending; release unrelated permits and rearm a durable wake.

## Shared effect contract

```text
prepared
  -> claimed(executor, attempt)
  -> dispatch_started
  -> response_started | detached(task_or_operation_id)
  -> result_ready | unknown
  -> settled
```

The tuple `(effect_id, attempt, executor)` is stable. A post-send transport
loss is unknown unless the same durable task/Hands operation can be queried.
Results over 32 KiB are content-authority references; DDB stores checksummed
metadata and a bounded preview only.

## Credential cache

Provider custody owns a process-local, zeroizing cache keyed by workspace,
credential, generation, provider scope, and policy/catalog revision. Cache
misses are single-flight and consume a bounded KMS permit. Revocation and
generation changes close exact pools and invalidate entries. The final strong
generation/revocation check remains immediately before provider send. Cache
configuration must enforce byte and TTL bounds rather than only entry counts.

## Scheduling and concurrency

The executor resource vector is `{restore_bytes, result_bytes, cpu_weight,
socket_weight, executor_weight, expected_work_seconds, vm_shape}`. Acquire
only the vector owed by the next dispatch phase. Capacity failure commits a
typed deferred continuation and releases the Brain claim.

Use hierarchical weighted deficit round-robin with age boost at region,
organization, workspace, session, generation, provider/server, credential, and
tool-class levels. Local pools are accelerators; durable reservations/fences
bound over-admission after task loss.

## Required tests

- typed inline tools cannot execute shell/network/filesystem paths;
- DDB conditional/idempotent todo transitions survive duplicate delivery;
- provider/MCP/web pool keys cannot cross tenant, credential, policy, or address
  generations;
- DNS rebinding and redirect screening remain enforced;
- dropped MCP task resumes by task identity;
- dropped non-task mutation settles unknown without retry;
- credential revocation closes pools and invalidates cache entries;
- MicroVM launch/resume parks and releases every permit;
- guest result receipt is checksummed and recoverable after Brain loss;
- concurrent independent tools preserve deterministic journal order;
- ARM64 guest boot, Bash, grep, package, file, isolation, and lifecycle smoke.

## Rollout

1. Keep tools fail-closed until production Brain composition has non-optional
   inline and MCP executors plus a hydrated catalog.
2. Run provider/web/MCP unit and integration tests with synthetic upstreams.
3. Run ARM64 container and MicroVM smoke in dev using immutable artifacts.
4. Enable typed inline tools first; enable network pools next; enable attached
   Hands only for already-running generations below pressure thresholds.
5. Preserve the previous immutable artifact for rollback; never replay an
   ambiguous external effect as part of rollback.
