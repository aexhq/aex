---
title: Performance-first architecture v1
description: ARM64-only performance, correctness, scalability, and reliability architecture for the Rust-native rewrite.
keywords:
  - architecture
  - performance
  - arm64
  - brain
  - tools
audience: implementation agents and maintainers
status: accepted
related:
  - references/rewrite/README.md
  - references/rewrite/brain.md
---

# Performance-first architecture v1

Status: accepted for implementation planning (prelaunch, ARM64-only clean cut)

This document records the architecture decisions to implement after review. It
does not change the wire contract by itself. The journal, lease/fence, effect
and receipt authorities remain the correctness boundary.

## Non-negotiable goals

- Keep the user-visible agent turn off Lambda.
- Optimize p95/p99 latency, throughput, and tail isolation before idle cost.
- Preserve Brain as the sole session/turn authority.
- Treat caches, pools, affinity, and SQS as performance mechanisms, never as
  authority.
- ARM64 is the only supported runtime architecture for this clean cut. Do not
  add x86 compatibility paths unless a measured blocker is found.
- A failed or ambiguous external effect is never blindly replayed.

## Runtime placement

```text
regional-session-api   warm ECS, one task at launch
regional-stream        warm ECS, one task at launch
brain-mux              warm ECS, one task at launch

regional-secret-api    isolated Lambda (public management only)
regional-observation-api
                       bounded Lambda for finite queries initially
regional-otlp          bounded Lambda admission/normalization
observation reconciler Lambda
export launcher        Lambda
export/heavy work      one-shot Fargate
```

The first production task count may be one per service to limit standing cost.
This is a known single-failure-domain launch mode. Durable leases, journal
state, and wake hints preserve correctness after task loss; availability and
in-flight latency improve when a second task is added. Scale-out must be
driven by queue age, active streams, reserved bytes, CPU-lane delay, and
executor saturation rather than CPU alone.

Brain may start at a smaller task shape only after ARM load evidence proves the
RSS and context-fold bounds. Session and stream tasks may start smaller. The
Brain concurrency/reservation model must be reduced with the task size; it may
not retain a 2-vCPU/4-GiB capacity claim on a smaller task.

## Brain activation path

```text
receive wake hint
  -> source pending-state read
  -> DynamoDB claim/fence
  -> strong session authority check
  -> exact-revision fold-cache lookup
  -> snapshot + journal suffix restore
  -> open-effect recovery
  -> model plan
  -> prepare durable effect
  -> dispatch-started/pre-send fence
  -> provider/MCP/Hands call
  -> settle durable result
  -> projection/preview side channels
```

Session reload and open-effect loading are independent authoritative reads and
should run concurrently after the claim. Restore memory is reserved before
claim; provider, network, MCP, and Hands permits are acquired only immediately
before their dispatch. If capacity is unavailable, commit a typed deferred
continuation, release the claim, and wake later. Never wait locally while
holding a session lease.

Preview delivery is bounded and asynchronous. A slow client cannot stall model
reads, provider settlement, or the durable journal.

## Tool fabric

### Brain-control lane

Only explicitly catalogued, bounded typed operations run inline:

- todo state;
- operation status;
- approval state;
- small session metadata;
- deterministic/idempotent DDB transitions.

Inline does not mean bypassing the journal or idempotency protocol. It means
using a typed executor instead of generic user-code execution.

### Network lane

Provider calls, `web_fetch`, `web_search`, and MCP Streamable HTTP remain direct
async operations with supervised logical permits. Pools are keyed by the exact
workspace/credential/origin/server revision and screened address set. DNS,
SSRF, redirect, and credential-generation checks remain per effect.

MCP manifests are frozen by revision. Safe independent tools may execute in
parallel, but durable commit order remains deterministic. Non-task mutations
that lose their response settle as ambiguous/unknown; they are not retried.

### Hands/MicroVM lane

Bash, `grep`, arbitrary scripts, file operations, package installation, and
workspace mutations always execute in the exact session-generation MicroVM.
There is no DDB fast path for shell operations. A warm generation is reused
while active; lifecycle launch/resume is a durable materialization operation.
The Brain parks, releases unrelated permits, and resumes on a durable wake.
Guest endpoint/token leases are cached by generation, fence, guest revision,
and token generation, and invalidated on lifecycle/fence/token/connect change.

### Heavy and detached work

CPU/memory-heavy transforms, large files, long external jobs, and exports use a
durable operation and an isolated worker (one-shot Fargate where appropriate).
The model parks on a continuation or returns a durable handle. Lambda is only
for non-critical background work that fits its limits.

## Secret and observation boundaries

Public secret management remains an isolated Lambda custody boundary. Provider
credential reveal stays inside warm Brain tasks; Brain does not call the public
secret API on every turn.

Credential caches are process-local, zeroizing, byte-bounded, and keyed by
workspace, credential, generation, provider scope, and policy/catalog revision.
Misses use single-flight and a bounded KMS permit. Revocation/generation change
invalidates the cache. A final authoritative generation/revocation fence is
required before send.

Observation is a projection/read surface, never Brain authority. Live/replay
uses regional-stream. Finite queries have explicit segment, item, byte,
decode-memory, output, and wall-time budgets. Large extraction is a one-shot
Fargate export. Heavy aggregates receive a separate concurrency lane. Do not
introduce ClickHouse or Redis until a dashboard-shaped load gate proves the
bounded DynamoDB/S3 design insufficient.

## Wake, scheduling, and scaling

Durable commit is the source of truth. When the owning Brain task is known,
use a direct in-process/task wake for latency, with DynamoDB Streams/EventBridge
and SQS as cross-task/recovery hints. SQS contains identifiers/hints, not
journals, prompts, credentials, or authoritative results. Domain queues remain
separate.

Admission uses hierarchical weighted deficit round-robin with age boost:

```text
region -> organization -> workspace -> session -> generation
       -> provider/server/credential -> tool class
```

The resource vector includes restore bytes, result bytes, CPU weight, sockets,
executor weight, expected work seconds, and MicroVM shape. Distributed permits
are durable/fenced; local permits are short-lived accelerators.

Rendezvous affinity is preferred for cache locality but never authority. A
task-loss or drain fallback may execute the durable claim on another task.

## ARM64 clean-cut qualification

All Lambda/OCI/ECS/Hands artifacts use ARM64. The exact artifact must execute
before certification. Required evidence:

1. ARM bootstrap execution for every Lambda unit.
2. ARM container startup and health execution for every OCI unit.
3. Hands image boot, package/tool checks, isolation, lifecycle, and guest RPC
   checks for every supported shape.
4. Brain 100/200/500 offered-load runs with p50/p95/p99, RSS, FD, stall, loss,
   and cost-per-completed-turn receipts.
5. Release admission requires an artifact-bound `arch-qualification` receipt.
6. Post-apply readiness verifies Lambda architecture and ECS task-definition
   architecture, not just image/code digest.

Customer MicroVM guest images are ARM64 for v1 as well. An x86 guest/image
fallback is deliberately out of scope for the clean cut; a dependency that
requires x86 is unsupported until a future architecture decision reopens it.

## Correctness invariants

- DynamoDB journal/effect/lease/fence state is authoritative.
- Cache loss changes latency only.
- Every park, failure, cancellation, and drain returns local permits.
- Prepared -> claimed -> dispatch-started -> result/unknown -> settled is
  durable and idempotent.
- A post-send network loss is unknown unless the same durable task can be
  queried.
- Plaintext secrets never cross tenant cache boundaries or enter logs/receipts.
- Projection lag is visible and cannot be used to reconstruct Brain context.
- SQS delivery is at-least-once and duplicate-tolerant.

## Implementation slices

1. **ARM/release qualification** — target execution, ARM receipts, readiness
   architecture checks, Hands ARM smoke, and canary/load evidence.
2. **Activation/cache/admission** — fold cache wiring, parallel restore reads,
   phase-specific permits, durable capacity deferral, and metrics.
3. **Production tools** — Brain-control executor, catalog hydration, MCP
   transport/pool, managed web pooling, ambiguity/task recovery.
4. **Provider/preview** — semantic deltas, bounded coalescing, connection
   reuse, exact credential-generation pool invalidation.
5. **Hands** — endpoint/token lease cache, durable materialization, attached
   bounded fast path, guest result receipts.
6. **Fleet** — one-task launch settings, scale signals, drain, rendezvous
   affinity, and later multi-task availability.
7. **Observation/secret hardening** — cache byte bounds, KMS single-flight,
   aggregate lane, projection lag/load gates, and export recovery.

## Known blockers before release

- The production Brain tool runtime and semantic preview path are incomplete.
- `session_create`/`message_send` serving must be verified before UX claims.
- ARM execution qualification and readiness checks are not currently enforced.
- Hands/MicroVM live evidence is unearned.
- The signed model-catalog authority/evidence gate remains open.
- The last public CI candidate failed while `regional-stream` was still private;
  it must not be reused. A fresh substantive public main push is required now
  that package visibility is public.

Savings Plans are deliberately deferred until steady usage telemetry exists.
