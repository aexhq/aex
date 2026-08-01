---
title: Brain core — what landed, what is published, what is deferred
description: Handoff for the brain-core stream of the Rust-native rewrite. Records the implemented slices of plan 07, the exact port trait signatures peers implement, the DynamoDB key namespace claimed inside session-authority, the changes needed from peer streams, the decisions taken beyond the orchestrator conventions, and the tracked remainder.
keywords:
  - brain
  - ports
  - fold
  - fence
  - effects
  - loom
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-01
related:
  - references/rust-native-rewrite-2026-07-31/plans/07-brain-core.md
  - references/rust-native-rewrite-2026-07-31/plans/00-orchestrator-conventions.md
  - references/rewrite/contracts.md
  - references/rewrite/test-architecture.md
---

# Brain core

Owned: `crates/aex-brain-domain`, `crates/aex-brain-application`,
`crates/aex-brain-store-aws`, `runtimes/brain-mux`, the `journal_gen` and
`histories` modules of `crates/aex-brain-test-support`.

## 1. Implemented

### `aex-brain-domain` — the pure half

No Tokio, no AWS, no clock, no randomness. Every timestamp and identifier is a
parameter, which is what lets every history in the suite replay exactly.

| Module | What it owns |
| --- | --- |
| `ids` | typed newtypes; deterministic `EffectId::derive` and `child_agent_id` |
| `canonical` | RFC 8785 JCS, bounded by depth and bytes, integers only |
| `journal` | the closed `JournalRecord` enum, envelope sealing, the bounded decoder |
| `fold` | `FoldState`, `apply`, `fold`, contiguity, fork detection, compaction |
| `planner` | `plan(state, policy) -> OwedStep`, total over every fold state |
| `context` | the pure view layer: trigger, clearing, per-result cap |
| `budget` | seven accumulating dimensions plus two structural, `I1`/`I2`, roll-up |
| `effect` | the split-phase state machine and the `recover()` matrix |
| `child` | child state, outcome, join status |
| `commit` | `DecisionCommit` and its envelope validator |
| `wire_pending` | minimal stand-ins for peer types, each with its replacement named |

Fold properties **F1–F12** are implemented as one test each over the
`proptest` generator, plus the hostile deliveries. 30 named golden histories
cover the semantic cases; roughly half are hostile and each is asserted against
the *named* guard rather than against "an error happened".

### `aex-brain-application` — ports and the kernel

Ten port traits (§2), plus the synchronization kernel that holds every atomic
and lock in the stream: `ActivationRegistry`, `PermitSet`, `WarmCacheShard`,
`DrainGate`, `RenewalState`, `CancelToken`. Loom models **L1–L6** run under
`--features loom`; the same six kernels also carry real threaded tests, so the
`concurrency` target is non-empty in both configurations and both lanes run.

### `aex-brain-store-aws` — namespace and expressions

`keys` claims the `BRAIN#` namespace (§3) and `expressions` describes every
transaction as data so the precondition sets are asserted without an AWS call.
`WorkExpressions` is the seam for the `regional-work` item shape.

### `runtimes/brain-mux`

Config validation (no defaults for anything that names a resource) and the
health contract: `/internal/healthz` and `/internal/readyz`, with the
superseded `/livez` asserted *not* to resolve.

## 2. Port traits published

All in `aex_brain_application::ports`. `BoxFuture` makes every port
dyn-compatible so the composition root selects adapters at runtime.

```rust
pub type BoxFuture<'a, T> = core::pin::Pin<Box<dyn core::future::Future<Output = T> + Send + 'a>>;

pub trait ProviderPort: Send + Sync + 'static {
    fn dispatch<'a>(&'a self, ticket: &'a DispatchTicket,
        request: &'a CanonicalModelRequest, budget: &'a StreamBudget,
        preview: &'a dyn PreviewSink, cancel: &'a CancelToken)
        -> BoxFuture<'a, Result<ProviderOutcome, ProviderDispatchError>>;
    fn resolve_unknown<'a>(&'a self, identity: &'a DurableEffect,
        evidence: &'a DispatchEvidence)
        -> BoxFuture<'a, Result<UnknownResolution, ProviderDispatchError>>;
}

pub trait ToolPort: Send + Sync + 'static {
    fn route(&self, pin: &CatalogPin, name: &ToolName) -> Result<ToolRoute, ToolRoutingError>;
    fn invoke<'a>(&'a self, ticket: &'a DispatchTicket, call: &'a PreparedToolCall,
        cancel: &'a CancelToken) -> BoxFuture<'a, Result<ToolOutcome, ToolDispatchError>>;
    fn query<'a>(&'a self, operation: &'a DetachedOperationId)
        -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>>;
    fn cancel<'a>(&'a self, operation: &'a DetachedOperationId, fence: Fence)
        -> BoxFuture<'a, Result<(), ToolDispatchError>>;
}

pub trait HandsPort: Send + Sync + 'static {
    fn ensure_generation<'a>(&'a self, session: &'a SessionId, generation: HandsGeneration)
        -> BoxFuture<'a, Result<HandsEndpoint, HandsError>>;
    fn start<'a>(&'a self, ticket: &'a DispatchTicket, generation: HandsGeneration,
        start: &'a HandsOperationStart) -> BoxFuture<'a, Result<HandsAccepted, HandsError>>;
    fn status<'a>(&'a self, generation: HandsGeneration, operation: &'a HandsOperationId)
        -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>>;
    fn cancel<'a>(&'a self, generation: HandsGeneration, operation: &'a HandsOperationId,
        fence: Fence) -> BoxFuture<'a, Result<(), HandsError>>;
    fn result<'a>(&'a self, generation: HandsGeneration, operation: &'a HandsOperationId,
        bounds: &'a ResultBounds) -> BoxFuture<'a, Result<HandsResult, HandsError>>;
}

pub trait CatalogPort: Send + Sync + 'static {          // sync: signed immutable artifacts
    fn digest(&self, pin: &CatalogPin) -> Result<CatalogDigest, CatalogError>;
    fn model(&self, pin: &CatalogPin, provider: ProviderId, model: &ModelSlug)
        -> Result<ModelCapability, CatalogError>;
    fn tool(&self, pin: &CatalogPin, name: &ToolName)
        -> Result<ToolManifestEntry, CatalogError>;
    fn durable_operation_support(&self, pin: &CatalogPin, provider: ProviderId,
        model: &ModelSlug) -> DurableOperationSupport;
}

pub trait ClockPort: Send + Sync + 'static {
    fn now(&self) -> Timestamp;          // epoch millis; durable records carry this
    fn steady(&self) -> SteadyInstant;   // monotonic; deadlines use this
}

pub trait IdPort: Send + Sync + 'static {
    fn child_agent_id(&self, parent: &AgentId, ordinal: u32) -> AgentId;   // deterministic
    fn effect_id(&self, agent: &AgentId, seq: JournalSeq, kind: EffectKind) -> EffectId; // ditto
    fn owner_token(&self) -> OwnerToken;
    fn wake_id(&self) -> WakeId;
    fn operation_id(&self) -> DetachedOperationId;
}

pub trait JournalStore: Send + Sync + 'static {
    fn load_head<'a>(&'a self, key: &'a AgentKey)
        -> BoxFuture<'a, Result<Option<AgentHead>, StoreError>>;
    fn read_page<'a>(&'a self, key: &'a AgentKey, from: JournalSeq, budget: ReadBudget)
        -> BoxFuture<'a, Result<JournalPage, StoreError>>;
    fn commit<'a>(&'a self, commit: &'a DecisionCommit)
        -> BoxFuture<'a, Result<CommitReceipt, CommitError>>;
}

pub trait EffectStore: Send + Sync + 'static {   // settlement is NOT here; see below
    fn mark_dispatch_started<'a>(&'a self, guard: &'a FenceGuard, effect: &'a EffectId,
        attempt: u16, at: Timestamp) -> BoxFuture<'a, Result<DispatchTicket, CommitError>>;
    fn mark_response_started<'a>(&'a self, ticket: &'a DispatchTicket,
        evidence: &'a DispatchEvidence) -> BoxFuture<'a, Result<(), CommitError>>;
    fn load_open<'a>(&'a self, key: &'a AgentKey)
        -> BoxFuture<'a, Result<Vec<DurableEffect>, StoreError>>;
}

pub trait LeaseStore: Send + Sync + 'static {
    fn claim<'a>(&'a self, key: &'a AgentKey, owner: OwnerToken,
        ttl: core::time::Duration, now: Timestamp)
        -> BoxFuture<'a, Result<Claim, ClaimError>>;
    fn renew<'a>(&'a self, claim: &'a Claim, ttl: core::time::Duration, now: Timestamp)
        -> BoxFuture<'a, Result<Claim, ClaimError>>;
    fn release(&self, claim: Claim, disposition: ReleaseDisposition)
        -> BoxFuture<'_, Result<(), StoreError>>;
}

pub trait WakeQueue: Send + Sync + 'static {     // delivery only; no `enqueue`
    fn receive(&self, max: usize, wait: core::time::Duration)
        -> BoxFuture<'_, Result<Vec<WakeDelivery>, StoreError>>;
    fn extend_visibility<'a>(&'a self, delivery: &'a WakeDelivery, by: core::time::Duration)
        -> BoxFuture<'a, Result<(), StoreError>>;
    fn release(&self, delivery: WakeDelivery, after: core::time::Duration)
        -> BoxFuture<'_, Result<(), StoreError>>;
    fn ack(&self, delivery: WakeDelivery) -> BoxFuture<'_, Result<(), StoreError>>;
    fn due_scan(&self, shard: WorkShard, now: Timestamp, max: usize)
        -> BoxFuture<'_, Result<Vec<DurableWake>, StoreError>>;
}
```

Two rules are carried by types rather than by review.

- **`FenceGuard`** is required by every store write. It is not the correctness
  mechanism — the durable precondition set is — but a caller cannot name an
  agent without one, so it cannot forget to condition its write on the fence
  it holds.
- **`DispatchTicket`** is minted only by `EffectStore::mark_dispatch_started`,
  from a `FenceGuard` plus the effect it belongs to. `dispatch`, `invoke` and
  `start` accept nothing else, so sending a byte before the durable pre-send
  write is a compile error. The ticket is deliberately not `Clone`: one
  pre-send write authorizes one attempt.

`EffectStore` carries **no settlement method** and `WakeQueue` carries **no
`enqueue`**. Settlement lands inside `DecisionCommit` so the outcome and its
journal record are atomic; wakes are created only there so the queue stays a
delivery hint rather than a second authority.

Also published: `DecisionCommit` and `validate()`, `CancelToken`,
`PreviewSink`/`NullPreviewSink`, `StreamBudget`, `RedactedDetail`,
`ProviderOutcome`/`ProviderDispatchError`/`UnknownResolution`,
`ToolRoute`/`PreparedToolCall`/`ToolOutcome`/`DetachedStatus`,
`HandsOperationStart`/`HandsAccepted`/`HandsOperationStatus`/`HandsResult`,
`AgentHead`/`Claim`/`CommitReceipt`/`ConditionFailure`/`StoreError`.

## 3. The `BRAIN#` namespace claim

Binding on the regional-stores peer. Inside `session-authority`:

- every Brain-owned **session-level** item uses the `BRAIN#` sort-key prefix
  under the `SESSION#<sid>` partition — `BRAIN#BUDGET`, `BRAIN#Q#…`,
  `BRAIN#FANOUT#…`;
- every **per-agent** item lives in its own `SESSION#<sid>#AGENT#<aid>`
  partition — `CONTROL`, `J#<seq:020>`, `E#<hex>`, `C#<ordinal:010>#<child>`,
  `JOIN#<id>`, `JOIN#<id>#S#<k:03>`, `MBOX#<seq:020>`, `P#<event_seq:020>`.

No other prefix is claimed. Sort keys are zero-padded so lexicographic order is
numeric order; a test enumerates every family and asserts each sits in exactly
one namespace.

## 4. What peers must supply

| Need | Owner | Note |
| --- | --- | --- |
| `aex_model_catalog::canonical` — `CanonicalModelRequest`, `CompleteAssistantMessage`, `NormalizedUsage`, `StopReason`, `PreviewFrame`, `ProviderReceipt`, `CanonicalBlock`, `ModelCapability`, `DurableOperationSupport` | providers | currently stood up in `aex_brain_domain::wire_pending`, each marked `TODO(cross-stream)`. Brain re-exports at merge, never redefines. |
| `aex_session_domain::journal::JournalEnvelope` plus the ordering algebra | regional domains | Brain's `wire_pending::JournalEnvelope` is `{seq, content_hash, recorded_at}`; the fold consumes it and owns payload interpretation only. |
| `aex_content_domain::ContentRef` | regional stores | `{hash, len, media_type, key, encryption}`. |
| `aex_brain_tool_catalog::ToolManifestEntry` | tools | must carry the `EffectClass` the effect driver reads. |
| `aex-work-dynamodb` pure expression builders usable inside a caller-owned `TransactWriteItems` | regional stores | Brain owns the `WorkExpressions` trait so adoption is one impl, not a rewrite. |
| `aex-session-dynamodb` run-terminal-barrier and session-head expression builders | regional stores | Brain executes that transaction. |
| `DispatchProof::NotSent` returned only with a tested, observable pre-dispatch failure | providers | it is the only value permitting an automatic retry, so an over-generous adapter creates a second generation. |
| `PreparedToolCall.control` carrying a `ControlStateView` | tools/MCP | already in the published shape so `todo_read` stays pure. |
| A `trybuild` workspace dependency, if a compile-fail fixture is wanted for the ticket rule | delivery | the rule is currently proved by the `ports` target compiling at all. |

## 5. Decisions taken beyond the conventions

| # | Decision | Why |
| --- | --- | --- |
| BR-01 | The canonical encoder's byte bound sits at the transaction ceiling (4 MiB), strictly above the 256 KiB item ceiling, enforced by a compile-time assertion | equal bounds made an over-large record surface as an undiagnosable `TooLarge`, so a caller could not tell "page this" from "this document is hostile" |
| BR-02 | The per-tool-result truncation marker is charged **against** the byte budget | appending it past the limit made a capped result `limit + marker` bytes, so a second pass cut it again and a retried request sent the model a different prompt |
| BR-03 | The Loom shim is selected by a Cargo feature, not `--cfg loom` | the flag form needs a `check-cfg` entry only the shared workspace lint table can carry, and no stream should edit a manifest it does not own |
| BR-04 | `Arc` is not shimmed for Loom; only `Mutex` and the atomics are | Loom's `Arc` is not a valid `self` receiver, and reference counting adds no interleaving of its own |
| BR-05 | The `concurrency` target holds real threaded tests *and* Loom models, mutually exclusive by configuration | keeps the target non-empty in both lanes without an `#[ignore]` or an env-var self-skip |
| BR-06 | Readiness inputs are named `Dependency` states, not bare booleans | four booleans at a call site say nothing about which is which, and readiness has to name the unsatisfied dependency |
| BR-07 | `EnvelopeViolation::ItemTooLarge` names the offending item, not the category | the caller has to decide whether to page or to place the body in the content authority, and "an item was too large" does not tell it which |
| BR-08 | A negative enqueue instant clamps to zero in the queued-child sort key | a skewed clock would otherwise sort ahead of every legitimate entry and jump the fair-share queue |
| BR-09 | A fork at a sequence **before** `base_seq` is an accepted no-op, not an error | compaction deliberately drops the hashes it absorbed, so such a record is indistinguishable from a redelivery of something already summarized. Stated as a property rather than left implicit. |
| BR-10 | The mutation gate asserts, for each guard, both the typed error **and** that the state did not move | a guard that rejects after a partial mutation is as broken as no guard: the caller reloads and finds a journal that absorbed the record it was told was refused |

## 6. Deferred, with what unblocks each

Everything below is tracked rather than silently dropped. None of it is
`#[ignore]`d or env-var skipped; the code for each simply does not exist yet.

| Slice | Deferred | Unblocked by |
| --- | --- | --- |
| S-2.3 – S-2.7 | the live DynamoDB adapter: transactions, paging, content placement, fault injection | the `aex-live-brain-mux` companion and the peer expression builders |
| S-3.1 – S-3.5, S-3.7, S-3.8 | wake dedup, due reconciliation, visibility management, poison policy, supervisor tree, drain execution | the store adapter and the Tokio composition root |
| S-4.x | the first vertical run | S-2 and S-3 |
| S-5.3 – S-5.6 | crash-window recovery controller, cancel path, preview writer | the effect driver over a real store |
| S-6.x, S-7.x | tool and Hands effect integration | `ToolPort`/`HandsPort` implementations |
| S-8.x | the subagent scheduler, paged fanout, join ledger, mailbox, tenant fairness | the store adapter's transaction execution |
| S-9.x | warm-cache lifecycle, pressure bands, admission, A11-MUX measurement | the Tokio composition root; `rustix` is **not** yet a workspace dependency, which the CPU-attribution work needs |
| S-10.x | destructive lifecycle and the deterministic history checker | a real store boundary |
| S-11 | `tests/load/brain/` workloads | the live companion and `aex-load-harness` |
| S-12 | the repository-boundary clean-cut tests | the TypeScript deletion pass |

The kernel primitives S-9 and S-3 need — `PermitSet`, `DrainGate`,
`WarmCacheShard`, `ActivationRegistry`, `RenewalState` — are implemented and
Loom-checked; what is missing is the Tokio composition that drives them.

## 7. Gate output

```
cargo fmt --all                                                    clean
cargo clippy -p aex-brain-domain -p aex-brain-application \
             -p aex-brain-store-aws -p brain-mux --all-targets \
             -- -D warnings                                        clean
cargo nextest run -p aex-brain-domain -p aex-brain-application \
                  -p aex-brain-store-aws -p brain-mux
    Summary [134.743s] 203 tests run: 203 passed, 0 skipped
cargo nextest run -p aex-brain-application --features loom \
                  --test concurrency  (LOOM_MAX_PREEMPTIONS=3)     6 passed
cargo check --workspace --all-targets                              clean
cargo run -p aex-workspace-check
    133 member(s) and 139 package(s) satisfy every structural and registry rule
```

## Second pass

The first pass landed the pure half and the synchronization kernel. This pass
landed the composition that drives them: the real store adapter, the subagent
scheduler, the Tokio composition root and the A11-MUX measurement, plus the load
campaigns and the live companion's targets.

### 8. What the second pass implemented

#### `aex-brain-store-aws` — the real adapter

`translate` is the one seam between `aex-brain-domain`'s plain `Uuid` newtypes
and the version-7 prefixed identifiers the regional key templates take. Every
crossing is fallible: an identifier that is not a valid version-7 payload is a
typed refusal, because a key built from a nonsense identifier addresses a
partition nothing else will ever address, so the write succeeds and is
invisible.

`keys` was rewritten to **delegate**. Six families — agent control, journal
entry, effect, fanout page, join shard, budget return — are declared and encoded
by `aex-session-dynamodb`, so every function for one of them calls that crate
rather than rendering a second spelling. Three families are Brain's alone and
sit in the two spaces the peer reserves: `BRAIN#` sort keys under
`SESSION#{sid}` for the session budget and the scheduler's queued index, and
`BRAINAGENT#{sid}#{aid}` partitions for the mailbox, child index and join group.

`plan::compile` turns one `DecisionCommit` into one `TransactionPlan` through
the workspace's single compiler, which refuses an unconditional authority write
and names every participant. Wakes ride through `aex_work_dynamodb::claim`'s own
expression builders, so Brain never forks that row shape.

`journal`, `effect`, `lease` and `wake` are the four ports over `aws-sdk-*`.
Request bytes are asserted with `capture_request`; page contiguity and fork
detection are asserted on decoded rows, because they are the two rules that
decide whether an agent may act at all and neither needs a service to observe.

#### `aex-brain-application` — the subagent scheduler

`subagent::fanout` plans a spawn as a pure function of the parent's fold, the
session's capacity and the request. `subagent::claim` acquires local permits
before the transaction and drops them on a deferral. `subagent::join` treats the
shard counters as a projection throughout. `subagent::mailbox` delivers in
mailbox order before the child's first model call, collapsing duplicates on the
caller's key.

#### `runtimes/brain-mux` — the composition root

`runtime` shapes the three schedulers; `control` is the health responder on its
own OS thread; `admission` returns `Admitted`, `Deferred` or a typed `Shed`;
`cache` holds the pressure bands and TTL-zero release; `scale` publishes twelve
signals and a predictive desired count; `drain` orders the seven shutdown
stages; `compose` refuses a configuration that does not hold together; `measure`
composes the usage probe.

`main::run` starts the control thread **before** the main runtime, so the
process can answer a probe before it can do anything else.

### 9. Decisions taken in the second pass

| # | Decision | Why |
| --- | --- | --- |
| BR-11 | §3's namespace claim is superseded in its *rendering*. The six families `aex-session-dynamodb` declares keep that crate's keys exactly; only the three families no peer declares take the reserved Brain spaces | the binding rule is one owner per physical item shape, and the peer landed those six with `AGENT#{sid}#{aid}` partitions and an `EFFECT#` prefix. Two renderers for one item is how a writer and a reader address different rows while both look correct |
| BR-12 | Brain compiles its own decision plan rather than calling `compile_decision` | that function takes one append, one effect and one event; a Brain decision carries vectors of each. It shares the key templates, the row codecs, the participant vocabulary and the one compiler, and a test asserts the single-append case produces exactly `DECISION_ORDER` |
| BR-13 | The agent's own budget movements ride inside the control update | two actions on one item are illegal in a `DynamoDB` transaction, and giving the node its own row would give it a second fence to keep in step with the first |
| BR-14 | `plan::compile` bounds the **compiled** plan against `MAX_ACTIONS` as well as running `DecisionCommit::validate()` | the compiled plan is one action larger than the domain's estimate because of the head guard. Both exist: the first tells a caller to page, the second is the number the service enforces |
| BR-15 | A settled effect row decodes as `OutcomeUnknown`, never as a specific outcome | the authoritative outcome is in the journal record the settlement was atomic with. An unrecognised state is a refusal rather than `Prepared`, because treating it as prepared is exactly how a possibly-sent request becomes a second generation |
| BR-16 | Capacity is consumed as a spawn page is planned | otherwise a page of 32 admits 32 children into one free slot. The first child is admitted and the rest carry the reason that is actually binding |
| BR-17 | Memory pressure **defers** where the safety cap **sheds** | the bytes come back and the work is admissible; an activation over the safety cap is not |
| BR-18 | The health responder writes its own HTTP/1.1 response rather than mounting the shared `axum` composition | it must share nothing with the request path it reports on. That is the same reason it has its own OS thread; a router here would be one more thing that can be slow exactly when the probe matters |
| BR-19 | `HEALTH_PORT` is a constant, not configuration | the ALB target group and the task definition both name it, and a defaulted-but-configurable port is a value two places can disagree about with no symptom until a deploy |
| BR-20 | Four gate rows were added to `release/policy/workload-registry.toml` | plan 07's Slice 11 table names the fanout and crash/restart gates, but the registry carried no row for either, so a descriptor implementing them failed `aex-workload-unowned`. The rows are brain-core's own obligations |

### 10. Still deferred, with what unblocks each

| Deferred | Unblocked by |
| --- | --- |
| The mux's wake loop — receive, dedup, claim, fold, plan, dispatch, settle, ack — and with it the first vertical run (S-4.x) | the queue URL and `regional-work` table bindings. `Config` deliberately still declares four variables; adding two more is a composition-manifest change the delivery stream owns |
| Live `DynamoDB`/S3/SQS behaviour: transaction conflict distributions, real paging, content placement, fault injection (S-2.3, S-2.5–S-2.7) | a deployed plane, or `aex_test_harness::DynamoDbLocalContainer` behind an `integration-engines` target |
| The effect driver's recovery controller, cancel path and preview writer (S-5.3–S-5.6) | `ProviderPort` implementations from the providers stream |
| Tool and Hands effect integration (S-6.x, S-7.x) | `ToolPort` and `HandsPort` implementations |
| Executing any load campaign | the wake loop plus a deployed plane. The descriptors, their gates and their metric sets are landed and asserted; the executor fails loudly rather than reporting a green run of zero work |
| Large-body content placement above 32 768 canonical bytes | `aex-content-aws`'s placement API, which Brain calls rather than writing S3 itself |
| Rendezvous affinity | `desired_count > 1`, which needs an affinity benchmark first (BC-25) |

### 11. Second-pass gate output

```
cargo fmt --all                                                    clean
cargo clippy -p aex-brain-application -p aex-brain-store-aws \
             -p brain-mux --all-targets -- -D warnings             clean
cargo nextest run -p aex-brain-domain -p aex-brain-application \
                  -p aex-brain-store-aws -p brain-mux
    Summary [124.705s] 355 tests run: 355 passed, 0 skipped
cargo nextest run -p aex-brain-application --features loom \
                  --test concurrency  (LOOM_MAX_PREEMPTIONS=3)     6 passed
cargo check --workspace --all-targets                              clean
cargo run -p aex-workspace-check
    133 member(s) and 139 package(s) satisfy every structural and registry rule
cargo run -p aex-workspace-check -- registry build                 regenerated
```

Child-count coverage: the fanout planner is asserted at 1, 5, 10, 15, 50, 100
and 200 children in the unit lane, including the page boundaries at 32 and 33.
The same seven counts are declared as shapes in `brain-swarm-fanout.toml` and
are asserted to be present there; executing that campaign needs a plane.
