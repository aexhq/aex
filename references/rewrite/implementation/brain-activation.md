---
title: Brain activation implementation plan
description: Test-first plan for the performance-first Brain activation path and ARM64 launch qualification.
keywords:
  - brain
  - activation
  - cache
  - performance
audience: implementation agents and maintainers
status: proposed
related:
  - references/rewrite/architecture-performance-v1.md
---

# Brain activation implementation plan

This plan implements only the accepted activation/cache/admission slice in
[`architecture-performance-v1.md`](../architecture-performance-v1.md), plus the
minimum provider, MCP, Hands, preview, release, and fleet seams needed to prove
it. It does not change the wire contract or create a second authority: the
DynamoDB journal, lease/fence, effect, and receipt records remain decisive.

## File ownership

| Owner | Responsibility |
| --- | --- |
| `crates/aex-brain-domain/` | Activation phases, typed deferral reasons, effect transition rules, resource reservations, and cache keys. No transport or storage calls. |
| `crates/aex-brain-app/` | Wake-to-settlement orchestration, concurrent restore reads, fold-cache use, phase-specific permits, park/resume, cancellation, and drain cleanup. |
| `crates/aex-brain-store-dynamodb/` | Pending-state reads; fenced claim/release; strong session, journal/snapshot, and open-effect loads; durable continuation/effect transitions. |
| `crates/aex-brain-provider-gateway/`, `crates/aex-brain-provider-custody/` | Pre-send permit and credential-generation fence, connection reuse, ambiguous provider outcomes, and credential/pool invalidation. |
| `crates/aex-brain-mcp/`, `crates/aex-brain-managed-web/` | Revision-keyed pools, per-effect network checks, bounded concurrency, deterministic result commit ordering, and unknown non-task mutations. |
| `crates/aex-brain-hands/` | Durable materialization park/resume and generation/fence/revision/token-keyed guest lease cache; no shell fast path outside Hands. |
| `runtimes/brain-mux/` | Wake intake, affinity routing, task drain, health, metrics, and composition of the owners above. It contains no durable transition policy. |
| `services/session-stream-api/` | Direct wake hints and bounded asynchronous preview consumption; neither becomes Brain authority. The session and stream services were one task from 2026-08-09. |
| `crates/aex-brain-test-support/`, `tests/` | Deterministic authorities/faults and cross-crate recovery, ambiguity, pressure, load, and task-loss scenarios. |
| `infra/`, `release/` | ARM64-only task/artifact declarations, one-task launch defaults, architecture qualification receipts, and post-apply architecture checks. |

Keep changes within these owners. Do not add an SQS payload authority, Lambda
turn path, x86 branch, or generic inline tool executor.

## Durable state transitions

```text
on wake(hint):
  pending = read_pending_state(hint.identity)       // hint is not authority
  reserve restore_bytes(pending.estimate)           // before taking a lease
  claim = claim_with_new_fence(pending.identity)

  concurrently after claim:
    session = strong_session_authority_read(claim)
    context = exact_revision_cache_get()
              or restore(snapshot, journal_suffix)
    effects = load_open_effects(claim)

  recover_open_effects_without_blind_replay(effects)
  plan = model_plan(session, context)

  for effect in deterministic_plan_order(plan):
    prepared = durable_prepare(effect, claim.fence)
    if dispatch_capacity_unavailable(effect.resource_vector):
      durable_commit(deferred_continuation(prepared, typed_reason))
      release_claim_and_all_local_permits()
      emit_recovery_wake_hint()
      return PARKED

    permit = acquire_immediately_before_dispatch(effect)
    durable_transition(prepared -> claimed -> dispatch_started)
    outcome = dispatch(effect)
    durable_transition(dispatch_started -> result | unknown -> settled)
    release(permit)
    enqueue_bounded_preview_and_projection(outcome)  // never awaited by settle

  release_claim_restore_reservation_and_all_permits()
  return SETTLED
```

Any cancellation, failure, task drain, or panic follows the same cleanup exit.
A failure before send may be retried through the durable state machine. A
post-send loss is `unknown` unless the same durable external task can be
queried; it is never blindly replayed. Duplicate wake hints re-read authority
and converge on the current fence/state.

## Test-first delivery slices

1. **State kernel.** In `aex-brain-domain`, first specify the complete legal
   transition table, stale-fence rejection, idempotent duplicate transitions,
   typed deferral, and `unknown` terminal recovery behavior.
2. **Activation orchestration.** In `aex-brain-app`, prove restore
   capacity precedes claim, session/context/open-effect reads overlap after the
   claim, exact-revision cache misses restore correctly, and no session lease
   is held while waiting for dispatch capacity.
3. **Durable adapter.** Against the real store contract, prove claim fencing,
   strong reads, snapshot-plus-suffix folds, task-loss recovery, duplicate wake
   tolerance, and cache-loss equivalence.
4. **Dispatch lanes.** Cover provider, managed web, MCP, and Hands permit
   timing; pre-send rejection; post-send ambiguity; safe queryable task
   recovery; revision/generation invalidation; and deterministic settlement of
   parallel safe tools.
5. **Backpressure and previews.** Saturate restore bytes, sockets, provider,
   executor, and Hands capacity independently. Assert durable park/re-wake and
   total permit release. A blocked or disconnected preview client must not
   delay provider reads or durable settlement.
6. **Recovery and fleet behavior.** Kill and drain `brain-mux` during restore,
   dispatch, settlement, and preview. Assert another task can take a new fence,
   no settled effect repeats, and affinity loss changes latency only.
7. **Production-shape acceptance.** Run exact ARM artifacts at 100, 200, and
   500 offered load and retain p50/p95/p99, RSS, file-descriptor, lane-delay,
   executor-saturation, stall, loss, and cost-per-completed-turn receipts.

Every slice lands only after its narrow unit/component tests and the relevant
cross-crate scenario are green. Load success does not waive a correctness
failure, and cache-hit performance does not waive cold/task-loss results.

## ARM64 launch sequence

1. Build every Lambda, OCI/ECS, and Hands artifact for ARM64 only; execute the
   exact immutable artifact, not a local substitute.
2. Earn bootstrap receipts for each Lambda, startup/health receipts for each
   container, and boot/tool/isolation/lifecycle/guest-RPC receipts for every
   supported Hands shape.
3. Pass the Brain 100/200/500 offered-load gates with the task shape's real
   reservation and concurrency limits. A smaller Brain task must carry smaller
   limits proven by the same evidence.
4. Bind the passing evidence to an `arch-qualification` receipt and make its
   absence or artifact-identity mismatch fail release admission.
5. Launch `session-stream-api` and `brain-mux` at one warm ARM task each. Record
   this as an accepted single-failure-domain launch mode;
   durable recovery is required, but uninterrupted availability is not claimed.
6. After apply, verify both immutable identity and declared runtime
   architecture for every Lambda and ECS task definition, then run serving,
   wake, task-loss, preview, and Hands canaries.
7. Scale on queue age, active streams, reserved bytes, CPU-lane delay, and
   executor saturation. Add a second task when availability or measured tail
   evidence requires it; never scale on CPU alone.

Launch remains blocked by incomplete production tools/semantic preview,
unverified session/message serving, missing ARM and Hands evidence, or an open
model-catalog evidence gate. There is no x86 fallback in v1.
