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
last_verified: 2026-08-03
related:
  - references/rewrite/contracts.md
  - references/rewrite/test-architecture.md
---

# Brain core

Plan of record: `references/rust-native-rewrite-2026-07-31/plans/07-brain-core.md` in the parent workspace.

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
    fn query<'a>(&'a self, operation: &'a DetachedOperationRef)
        -> BoxFuture<'a, Result<DetachedStatus, ToolDispatchError>>;
    fn cancel<'a>(&'a self, operation: &'a DetachedOperationRef, fence: Fence)
        -> BoxFuture<'a, Result<(), ToolDispatchError>>;
}

pub trait HandsPort: Send + Sync + 'static {
    fn ensure_generation<'a>(&'a self, session: &'a SessionId, generation: GenerationId)
        -> BoxFuture<'a, Result<HandsEndpoint, HandsError>>;
    fn start<'a>(&'a self, ticket: &'a DispatchTicket, generation: GenerationId,
        start: &'a HandsOperationStart) -> BoxFuture<'a, Result<HandsAccepted, HandsError>>;
    fn status<'a>(&'a self, generation: GenerationId, operation: &'a HandsOperationId)
        -> BoxFuture<'a, Result<HandsOperationStatus, HandsError>>;
    fn cancel<'a>(&'a self, generation: GenerationId, operation: &'a HandsOperationId,
        fence: Fence) -> BoxFuture<'a, Result<(), HandsError>>;
    fn result<'a>(&'a self, generation: GenerationId, operation: &'a HandsOperationId,
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
    fn read_page<'a>(&'a self, key: &'a AgentKey, from: JournalSeq, budget: ReadBudget,
        after: Option<JournalCursor>)
        -> BoxFuture<'a, Result<JournalPage, StoreError>>;
    fn commit<'a>(&'a self, commit: &'a DecisionCommit)
        -> BoxFuture<'a, Result<CommitReceipt, CommitError>>;
}

pub trait EffectStore: Send + Sync + 'static {   // settlement is NOT here; see below
    fn mark_dispatch_started<'a>(&'a self, guard: &'a FenceGuard,
        authority: &'a SessionAuthority, effect: &'a EffectId, attempt: u16, at: Timestamp)
        -> BoxFuture<'a, Result<DispatchTicket, CommitError>>;
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
    fn state<'a>(&'a self, wake: &'a DurableWake)
        -> BoxFuture<'a, Result<WakeState, StoreError>>;
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
  from the claimed session authority, a `FenceGuard` and the effect it belongs
  to. `dispatch`, `invoke` and `start` accept nothing else, so sending a byte
  before the durable pre-send transaction is a compile error. That transaction
  condition-checks session lifecycle/cancellation/deletion and current agent
  ownership while moving the effect. The ticket is deliberately not `Clone`:
  one pre-send transaction authorizes one attempt.

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
| `aex_model_catalog::canonical` — `CanonicalModelRequest`, `CompleteAssistantMessage`, `NormalizedUsage`, `StopReason`, `PreviewFrame`, `ProviderReceipt`, `CanonicalBlock`, `DurableOperationSupport` | providers | **Corrected 2026-08-01.** Landed, but every one has diverged from the copy in `aex_brain_domain::wire_pending`, and Brain never took the dependency. Adoption is a fold and journal-encoding change, not a re-export. `ModelCapability` does not exist there at all: the catalog describes a model with `document::ModelEntry` over `document::ModelLimits` and `document::CapabilitySet`. |
| A journal envelope | regional domains | **Corrected 2026-08-01.** `aex-session-domain` publishes no envelope. It publishes `journal::JournalEntry`, keyed by agent and carrying a typed kind, an `EntryIdentity`, a `JournalBody` and an `AuthorityFact`, where Brain's `wire_pending::JournalEnvelope` is `{seq, content_hash, recorded_at}`. |
| A content reference | regional stores | **Corrected 2026-08-01.** `aex-content-domain` publishes no `ContentRef`. Its nearest type is `descriptor::ContentDescriptor`, which is workspace-scoped and carries a `Placement` and a `CiphertextIdentity` rather than `{key, encryption}`. |
| `aex_brain_tool_catalog::ToolManifestEntry` | tools | **Corrected 2026-08-01.** It exists and carries the `EffectClass`, but `aex-brain-tool-catalog` already depends on `aex-brain-domain`, so importing it there is a dependency cycle. The concept has to move down or out. |
| `aex-work-dynamodb` pure expression builders usable inside a caller-owned `TransactWriteItems` | regional stores | **Corrected 2026-08-02.** `claim::enqueue`, `enqueue_dedupe`, and `complete_pending_agent_wake` now supply the three work actions Brain composes. The last one accepts only the exact pending `agent.wake` at work fence zero and removes both sparse due-index keys under the caller's session/agent transaction guards. |
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
| S-11 | `tests/load/workloads/brain-core/` workloads | the live companion and `aex-load-harness` |
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

## Third pass

The first pass landed the pure half and the synchronization kernel; the second
landed the store adapter, the subagent scheduler and the composition's shape.
This pass landed the loop that drives them.

### 12. What the third pass implemented

#### `aex-brain-application::activation` — the loop

`activation` was an empty placeholder. It now holds the whole cycle:

```
due shard + receive -> verify source -> dedupe -> local slot -> claim -> recover
        -> fold -> plan -> dispatch -> settle -> commit -> retire source
        -> release lease -> ack hint
```

- `decide` is the pure half: one owed step in, one `DecisionCommit` out. Every
  settlement writes both the journal record and the durable effect row, so an
  outcome and the record it produced land in one transaction.
- `run` is the asynchronous half. Nothing in it names a runtime; every wait is a
  port call, which is what lets the whole loop be asserted with a block-on that
  *panics* on `Pending`.
- `memory` publishes in-memory ports behind a `testing` feature. They refuse
  rather than invent — the provider never fabricates a generation, the store
  enforces the whole precondition set — so the crash-boundary assertions are not
  vacuous. `brain-mux` consumes the same fixtures, so the engine and its
  composition are asserted against one behaviour rather than two.

The crash boundaries below are asserted by interrupting a run at a named durable
point and running a second activation against what the first left:

| Interrupted at | Asserted |
| --- | --- |
| after the pre-send write, before the settlement commits | the effect settles `OutcomeUnknown`, the run terminalizes `interrupted`, and the provider is dispatched exactly once |
| after the commit, before the ack | the redelivery finds a terminal agent, acks, and does not run the turn again |
| before the source-retirement decision | the due row survives, a second owner retires it, and no effect is dispatched twice |
| after source retirement, before the queue ack | the strong source read observes `done`, the duplicate hint is acked, and no agent claim is needed |
| the commit loses its fence | nothing is appended, no byte leaves, the delivery is released and never acked |
| cancellation or deletion after `EffectPrepared`, before ticket mint | the session-head participant loses the pre-dispatch transaction, the effect stays prepared and zero external dispatch occurs |
| the page read observes a gap | the agent does not fold, does not plan, does not act and does not ack |

The source-retirement rows were introduced red-first; the earlier rows retain
their mutation proof.

#### `runtimes/brain-mux` — the composition

`Config` grows the two variables §10 named. `wake` resolves each port to an
adapter or refuses it by name; `pump` receives, recovers due work and drives a
bounded concurrent batch until drain; `drain_sequence` walks all seven stages
and joins or explicitly aborts-then-joins the receive task, so the process
proves every admitted activation stopped rather than merely intending to stop.

A11-MUX is asserted through the composition and not only inside the probe: the
loop's future is metered across a provider that is pending for a scripted 999 ms
and none of it is attributed.

### 13. Decisions taken in the third pass

| # | Decision | Why |
| --- | --- | --- |
| BR-21 | Recovery runs before the planner, on every activation | the planner has no dispatch evidence and its `Effecting` arm cannot distinguish a prepared model call from a prepared tool call. Letting it decide a dispatched effect would be a guess with a bill attached |
| BR-22 | A `NotSent` provider failure settles `KnownFailure` **and** prepares the replacement in the same commit | the fold moves a settled effect to `AwaitingFinish`, so settling alone would have the planner report `Completed` for a call that never happened. The replacement is what keeps the fold honest, and `max_provider_attempts` is what keeps it bounded |
| BR-23 | An effect already `Prepared` is dispatched, never re-prepared | the planner returns `ModelCall` for a prepared effect and a fresh one alike. Preparing a second would charge the reservation twice and leave the first identity open for the agent's life |
| BR-24 | A failed ack releases the delivery instead of consuming it | the decision has already committed; consuming the delivery would strand an agent with work owed and nothing to wake it. This was a real defect the boundary test caught |
| BR-25 | Admission stops receiving entirely while any binding is unsatisfied | a task that took deliveries only to release them would, after `max_receives` redeliveries, have the poison policy ack a wake nothing ever served. Not receiving is the only behaviour that cannot lose work |
| BR-26 | A tool that could not be dispatched is recorded as a `ToolResult` with `is_error`, not as a terminal | the alternative ends a whole session because one optional tool was unavailable, and the manifest already says a failed tool is a result the model decides about |
| BR-27 | `mark_response_started` writes every attribute `effect::decode` reads back | the decoder reads the closed external-operation or detached-id-plus-executor binding, `providerRequestId` and `receiptHash`. Omitting any half of a detached binding would make restart recovery either impossible or route-ambiguous. |
| BR-28 | The wake loop's step bound counts **committed decisions**, not planner steps | it exists to stop one activation holding a lease indefinitely, and a lease is held across commits. The planner's own limits are what stop a run |

### 13.1 Decision taken in the continuation pass

| # | Decision | Why |
| --- | --- | --- |
| BR-29 | Workspace, organization and deletion epoch are read from the claimed session head and passed explicitly into every decision commit; a wake's tenant is only a projection assertion | a mux serves many sessions, so fixing any of these facts at process construction can silently write one tenant's rows under another tenant. All three are rechecked by the transaction's session-head condition, so a mismatched tenant or a trash/purge racing the activation refuses the whole decision. |

### 13.2 Decisions taken in the wake-reliability pass

| # | Decision | Why |
| --- | --- | --- |
| BR-30 | A bounded rotating `regional-work/gsi_due` scan is the queue's durable backstop; recovered rows carry `WakeOrigin::DueScan`, never a synthetic receipt | the stream and SQS are delivery hints, so a lost hint must not strand authority. Explicit provenance makes visibility, poison counting and ack no-ops for a scan result instead of issuing an invalid queue call. Sixty-four shards matches the strict-v1 table descriptor; BR-35 and BR-41 fix the current receive/scan/drive ordering and bounds. |
| BR-31 | Every successful or already-terminal agent activation ends with a retirement-only `DecisionCommit`: the session head and agent control are condition-checked under the live claim, while the exact pending source wake is moved to `done` and loses both due-index keys in the same transaction | a separate `UpdateItem` cleanup would have no agent fence, and acking first would lose the recovery path. The pure retirement does not manufacture a journal tail or advance the revision; terminal agents remain claimable solely so this transaction still has a live fence. |
| BR-32 | A retirement-only transaction hashes agent, revision, tail and source work identity into its own 36-character client request token | it follows the final journal decision without advancing that decision's tail. Reusing the tail-only token with different transaction parameters would make DynamoDB reject the retirement as an idempotent-parameter mismatch. |
| BR-33 | Hands effects bind the canonical runtime `aex_wire::ids::GenerationId` read from session authority; Brain has no local numeric generation or unbound-generation state | runtime idle recount and recovery select the exact generation that admitted an operation. `AgentControl`, child fanout, `AgentHead`, `HandsPort`, pinned agent config and Hands effect rows now carry the same typed UUID. |
| BR-34 | One structured activation supervisor races claimed work against a five-second timer; each renewal atomically rechecks session lifecycle, cancellation, deletion and current agent ownership before extending the 15-second lease, then extends a live SQS receipt to 30 seconds | provider, managed-tool and Hands waits may last 600 seconds. Ownership or session-authority loss sets the adapter cancellation token and drops the pending work under its existing fence. Drain also sets the token; returned dispatch proof, never the token, decides whether the effect settles unknown or may be safely re-armed. A visibility failure permits a duplicate hint instead of hiding work whose ownership is uncertain. |
| BR-35 | SQS receive remains first, but the cadence-limited rotating due burst completes immediately after that receive and before any external effect is polled; queue and recovered deliveries then share one bounded concurrent scheduler | a failed receive cannot advance a due cursor, and ten 600-second effects no longer serialize into roughly 100 minutes or delay lost-hint recovery until they finish. Every 20 seconds the backstop queries 16 of 64 shards and takes at most one exceptional lost-hint row per shard. Admission and the explicit drive bound cap aggregate context, stream and future state. |

### 13.3 Decisions taken in the pre-send-integrity pass

| # | Decision | Why |
| --- | --- | --- |
| BR-36 | Ticket minting reuses the decision compiler's canonical session-head condition as the first participant in the pre-dispatch transaction; the following participants prove current agent owner/fence and move the exact prepared effect | `EffectPrepared` is not dispatch authority. Cancellation, trash or purge can commit after preparation, and a separate pre-send read would leave another race while adding latency. One `ConditionCheck` makes the session fact and effect transition share the serialization point. |
| BR-37 | A due row becomes a wake only when its base key, derived shard and effective due position all match `workId`, `dueAt` and priority. Invalid rows are skipped while the native cursor advances; each page returns the full invalid count plus at most eight closed-reason, key-digest diagnostics | trusting projected fields lets a forged past index key wake future work or address a different base row. The delivery port cannot delete or rewrite work authority, so logical isolation plus a bounded redacted diagnostic is the only boundary-correct quarantine; it never carries tenant identifiers or row keys. |

### 13.4 Decisions taken in the tail-and-restore-budget pass

| # | Decision | Why |
| --- | --- | --- |
| BR-38 | Journal pagination carries `DynamoDB`'s complete native `LastEvaluatedKey`; every `pk`/`sk`, partition, journal sort key and resume sequence is decoded and revalidated, and activation accepts a restore only when its final `(sequence, content hash)` equals the pair returned in the claimed `AgentHead` | `DynamoDB` may return a short page with a continuation, so entry count cannot distinguish EOF. Sequence alone also cannot distinguish a same-tail fork. Either ambiguity reaching recovery or planning can dispatch from a prefix or a different history. |
| BR-39 | Cold restore has strict activation-wide entry and canonical inline-body-byte ceilings in addition to per-page bounds; mux admission reserves a separate conservative decoded-restore allowance before restore and releases it by RAII with the activation | many valid pages are still unbounded in aggregate, and payload bytes understate decoded Rust plus canonicalization memory. The code-owned [`ActivationPolicy::default`](../../crates/aex-brain-application/src/activation/mod.rs) bounds are inclusive across all pages. Model token limits are not memory measurements and are not used. The current 64 MiB reservation covers the measured four-megabyte-body amplification and conservatively extrapolates through the eight-megabyte launch ceiling; it must be retuned only from reproducible measurements. |

### 13.5 Decisions taken in the runtime-liveness pass

| # | Decision | Why |
| --- | --- | --- |
| BR-40 | Prepared-effect takeover mints a ticket only for the attempt already stored on the durable effect and never writes a replacement attempt number | ownership takeover is not a retry: the predecessor sent no byte. Rewriting the attempt would mutate durable history and make settlement evidence describe a generation that never existed. |
| BR-41 | Drain retains the exact pump join handle across the cooperative timeout; expiry explicitly aborts and joins it, and `Stage::Exit` is refused while any drain permit remains | dropping a timed-out join handle detaches the pump. Readiness can fail at drain start, but a clean exit is true only after every admitted activation quiesced or its structured parent was cancelled and joined. |
| BR-42 | Drain and session-authority loss set the downstream cancellation token; an ambiguous dispatch settles unknown, while only `NotSent` may create a replacement effect and continuation wake in one commit | cancellation in one process cannot unsend a request. Proof-preserving settlement prevents duplicate billing, and atomic re-arm prevents shutdown from stranding a prepared effect without a wake. Every late write retains the original session and agent fences. |
| BR-43 | One receive pass scans due shards before driving anything, deduplicates queue/due hints, and polls all deliveries through `max_concurrent_drives` unordered futures | recovery latency is independent of effect duration, ten long effects overlap instead of serializing, and the explicit bound caps runnable futures and their reserved memory/CPU. |
| BR-44 | A due page advances its native cursor only when every valid wake reached a stable outcome; a release, refusal or scan fault retains the prior cursor, while the shard rotor advances independently | a cursor is an assertion that earlier work was handled. Transient work must be revisited, but one hot or malformed shard must not starve later shards. |
| BR-45 | SQS decoding returns valid and malformed siblings separately. Each malformed record is released below `max_receives` and acknowledged only at or above that threshold | one poison body cannot reject or repeatedly hide nine valid messages, and poison handling is an explicit per-record policy rather than an accidental batch error. |
| BR-46 | Every poll carries the complete due-isolation count plus at most eight closed-reason, sixteen-hex fingerprints into the host's structured telemetry event | discarding the adapter's bounded diagnostics made logical quarantine operationally invisible. The event contains no tenant, session, work id, row key or body. |

### 13.6 Decisions taken in the stateless detached-recovery pass

| # | Decision | Why |
| --- | --- | --- |
| BR-47 | A detached tool is durably named by `DetachedOperationRef { id, executor }` in both effect evidence and the journal wait; recovery refuses if those bindings disagree | upstream ids are executor-scoped. A process-local id-to-route map disappears on restart and lets equal raw ids from two executors overwrite each other. The composed router now indexes four fixed executor slots directly and holds no per-operation state or lock. |
| BR-48 | `DetachedStatus::Unknown` means an authoritative durable-absence response. Read-after-accept propagation lag and retryable query transport failures re-arm the existing wait until the effect's persisted deadline | treating eventual-consistency lag as absence interrupts valid work; retrying from a process-local deadline can poll forever after restart. Poll wakes remain delivery hints created only in `DecisionCommit`, with a non-zero scheduler floor so a zero hint cannot hot-loop. |
| BR-49 | A completed or definitively failed detached query settles the effect, resolves its existing wait, records the exact executor result, and creates the continuation wake in one `DecisionCommit` | handing back after only the result commit strands the next model call. A separate enqueue would make the queue a second authority and reopen the commit/ack crash window. |
| BR-50 | Detached operation identity is persisted before its journal wait. A successor that finds the response-started ToolCall effect in the narrow inter-write state revalidates the ordered pending call, request hash, class and pinned executor, then reconstructs the wait without external I/O; an already-expired operation opens and closes the wait plus settles unknown atomically | persisting the wait first can leave an unresolvable operation-less wait, while requiring the operation write to retain current-owner authority can lose an already-accepted upstream identity after lease theft. The repair is restart-only, never redispatches and leaves the successful hot path unchanged. |

### 13.7 Decisions taken in the verified-fold-snapshot pass

| # | Decision | Why |
| --- | --- | --- |
| BR-51 | A fold snapshot is derived acceleration only: the journal and agent control remain the sole semantic authority, while a separate `FoldSnapshotStore` exposes immutable body read plus one strongly selected per-agent pointer. Body read/publish receives the `WorkspaceId` already returned by this activation's claimed session authority | regional content keys are workspace-scoped, but putting tenant/session authority into `brain-mux` process configuration would reduce horizontal throughput and create process-local correctness state. The workspace argument is request-scoped, and the in-memory fixture rejects a mismatch. A mux may lose every cache and restart without changing an answer. |
| BR-52 | Snapshot body `aex.brain.fold.v1` is uncompressed canonical JCS. Its pointer and body repeat the exact agent, absorbed `(JournalSeq, BLAKE3 journal hash)`, SHA-256 config digest, SHA-256 body digest and byte length; restore verifies every equality and re-canonicalizes before using state | sequence alone cannot detect a same-tail fork, and trusting a body merely because its object key matches lets schema/key/config substitution reach planning. Uncompressed bytes avoid decompression bombs and a second decoded-size/CPU bound. Typed-key maps encode as sorted `[key,value]` arrays because JSON object keys can only be strings. |
| BR-53 | A valid snapshot seeds only a native-cursor journal suffix and every success still proves the exact head pair returned by the fenced claim. Missing, unavailable, corrupt, ahead-of-claim or same-sequence-fork snapshots get one bounded sequence-zero fallback; a present/degraded snapshot plus fallback failure preserves both typed causes, while ordinary pointer absence preserves the pre-existing authoritative replay error | the optimization never becomes authority and agents that have not reached the snapshot threshold retain their old failure classification. A valid snapshot whose suffix itself exceeds the ceiling does not attempt a strictly larger sequence-zero replay. Domain fold failures remain `FoldError`, not storage decode errors. |
| BR-54 | Pointer publication rejects an unknown schema or impossible body length before I/O, condition-checks the exact immutable historical journal row, proves current control tail is at least the cut, then advances only from an absent row or a correctly typed/identified pointer to a greater sequence, accepting only the exact same cut/body/config/schema retry | requiring the snapshot cut to equal current head would starve continuously active agents; allowing an unchecked/regressing pointer or silently healing a wrong-type row would hide corruption and let a slow publisher roll state backward. The immutable body must be placed through regional content authority first—Brain never writes S3 directly. |
| BR-55 | Snapshot bodies are capped before allocation and valid body bytes share the activation-wide suffix byte ceiling. Admission reserves a separate 64 MiB restore working set; the `snapshot_memory` example builds four 1,000,000-byte text blocks in one process and verifies the resulting 4,002,058-byte body in a fresh process | JSON decode plus JCS verification temporarily holds multiple representations. On Windows debug binaries the verify-only reported peak delta was 17,141,760 bytes over a 4,968,448-byte baseline (about 4.28× body bytes). The four-bytes-per-token shape is a planning approximation, not tokenizer truth; 64 MiB conservatively covers linear extrapolation through the 8 MiB launch ceiling plus suffix/accounting headroom, but does not become a semantic context limit. |
| BR-56 | The optional snapshot-body cache is keyed by `(WorkspaceId, SHA-256 immutable identity)`, verifies the digest before insert, refuses same-digest distinct bytes, has independent entry/aggregate byte ceilings and LRU eviction, and stores no mutable pointer | a cache may improve latency only. Workspace in the key makes cross-tenant reuse impossible by construction; the adapter must still authorize the request-scoped workspace before lookup. Keeping it below the future store adapter means mux stays process-only; cache clear, eviction and process restart are ordinary misses. It is not wired until the real content adapter exists and a workload measurement proves benefit. |
| BR-57 | `brain-mux` compiles its CPU and memory shape from its unique `rust-oci-service` row in `release/units.toml`; the build refuses a missing, duplicate, wrong-package, non-service or malformed Fargate row | the previous composition hard-coded 2 GiB while the release authority deployed 4 GiB, so tests proved a process that would never run. Binding the constants at build time keeps release shape authoritative without parsing TOML or installing mutable task state on the hot path. |
| BR-58 | Admission atomically acquires one RAII bundle containing activation, 64 MiB restore, 1 MiB stream-buffer, one provider-stream and one Hands-RPC reservation before claim or any body/page read; `should_receive` checks the exact same bundle | acquiring resources independently permits split capacity under concurrency, and waiting for provider/Hands capacity after claim holds a durable lease while no progress is possible. Reserving both external paths for the activation lifetime is conservative and may leave a permit unused, but eliminates post-claim local waits and preserves the no-unreserved-read/OOM guarantee. |
| BR-59 | The 4096 MiB task is split into 3072 MiB context, 128 MiB stream buffers, 512 MiB warm cache and 384 MiB unavailable headroom. Provider and Hands pools each hold 48. Composition rejects a target above the minimum of every resource capacity and scheduler width; the launch target and outer scheduler width are 48, while each receive scope still owns one drive | 48 simultaneous worst-case restores consume exactly 3072 MiB. A scheduler width of 10 silently capped throughput far below the declared target; a width above 48 would advertise work the context pool cannot retain. One-delivery scopes prevent nested batch fan-out from multiplying the process cap. |
| BR-60 | Restore scratch is not yet shrunk to a smaller retained-context reservation after hydration | there is a reproducible upper bound for peak decode/verify RSS but not for the allocator-retained folded state. Releasing the 64 MiB reservation from a guessed payload ratio could admit the next body while the first allocation remained resident. The conservative full-lifetime reservation costs concurrency only beyond the proven target; revisit after a fresh-process retained-RSS campaign publishes a reproducible bound. |

### 14. Still deferred, with what unblocks each

| Deferred | Unblocked by |
| --- | --- |
| Complete production peer set | provider custody/router and the signed immutable catalog collection are composed; concrete tool executors and the Hands runtime backend remain absent, so readiness and receive admission remain closed |
| Concrete Hands runtime backend | `aex-brain-hands::HandsAdapter` now implements `HandsPort` and enforces response generation equality, but no crate implements its `HandsBackend` over the runtime-activity store plus authenticated guest transport |
| A `ToolExecutor` for any route | `aex-brain-managed-web` and `aex-brain-mcp` implement none, so the composed router is linked with zero executors and refuses by its own typed error |
| The recovery controller's `RetrySameEffect` on a *dispatched* effect | nothing moves a dispatched effect back to `prepared`, so the arm is a named refusal. Unreachable for the classes this loop prepares, which a test asserts |
| `ReconstructFromReceipt` | `aex-content-aws`'s placement API: the receipt is a digest, and the body it names lives in the content authority |
| `OwedStep::SpawnChildren` | the `create_subagent` tool, which is the only thing that produces a fanout request for `subagent::plan_spawn`. The planner has no arm that reaches it today |
| Production restore above the activation-wide entry/byte ceiling | the domain codec, restore path, monotonic DynamoDB pointer plan and fail-closed mux binding now exist. Production remains deliberately unready until regional content authority implements immutable snapshot body publish/read and a trusted publisher enforces the active fleet's restore byte ceiling before invoking the pointer plan after the body is durable; Brain will not write S3 directly or substitute an unbound process cache |
| Successful snapshot-fallback telemetry | `restore` retains the typed `RestoreSource`, including degraded/corrupt/ahead/fork diagnostics, but the current `Session::reload` consumes only the verified state. Wire a bounded observation sink with the production content adapter before claiming fallback-rate or snapshot-hit operational visibility; terminal dual failures already preserve both causes in `ActivationError` |

### 14.1 BR-33 canonical generation and production injection

The session authority writes `generationId` as a canonical `gen_…` identifier on every
agent control row, including child fanout. Brain's claim decoder requires that attribute and
returns it on `AgentHead`; it does not default, derive or convert it.

Prepared effects carry `generation: Option<GenerationId>` in memory and `generationId` on
the DynamoDB row. `DecisionCommit::validate` accepts the field only for
`EffectKind::HandsOperation`, requires it for that kind, and rejects it for all others. The
effect decoder applies the same rule and rejects missing, malformed or unexpected bindings.
The physical recount contract is therefore closed:

| Attribute | Exact value |
| --- | --- |
| `itemType` | `agent_effect` |
| `kind` | `HandsOperation` |
| `generationId` | canonical `aex_wire::ids::GenerationId` string |
| open `state` | `prepared`, `dispatched`, `responding` |
| terminal `state` | `settled`, `unknown` |

`brain-mux` exposes `ProductionPeers`, whose constructor requires provider, tool, catalog and
Hands backend peers together and wraps the Hands backend in
`aex_brain_hands::HandsAdapter`. The current executable composes the real provider router and,
only when the complete build-bound collection verifies and contains an `Active` model, the
immutable catalog port. The collection covers every still-live session pin explicitly; it is
not newest-only or last-N. Missing real publisher roots/artifacts, any invalid revision, or a
cryptographically valid zero-`Active` collection leaves the catalog binding unready. Tool
executors and the concrete Hands backend are still absent, so the process receives no work.

### 15. Third-pass gate output

```
cargo fmt -p aex-brain-application -p aex-brain-store-aws -p brain-mux  clean
cargo clippy -p aex-brain-application -p aex-brain-store-aws \
             -p brain-mux --all-targets -- -D warnings                 clean
cargo nextest run -p aex-brain-domain -p aex-brain-application \
                  -p aex-brain-store-aws -p brain-mux
    Summary [311.890s] 384 tests run: 384 passed, 0 skipped
cargo nextest run -p aex-brain-application --features loom \
                  --test concurrency  (LOOM_MAX_PREEMPTIONS=3)         6 passed
cargo check --workspace --all-targets                                  clean
cargo run -p aex-workspace-check
    133 member(s) and 140 package(s) satisfy every structural and registry rule
cargo run -p aex-workspace-check -- registry build      no change to either file
```

### 16. Continuation-pass gate output

```text
cargo fmt -p aex-brain-application -p aex-brain-store-aws -p brain-mux  clean
cargo clippy -p aex-brain-application -p aex-brain-store-aws \
             -p brain-mux --all-targets -- -D warnings                 clean
cargo nextest run -p aex-brain-domain -p aex-brain-application \
                  -p aex-brain-store-aws -p brain-mux
    Summary [136.307s] 388 tests run: 388 passed, 0 skipped
cargo nextest run -p aex-brain-application --features loom \
                  --test concurrency  (LOOM_MAX_PREEMPTIONS=3)
    Summary [21.599s] 6 tests run: 6 passed, 0 skipped
cargo check --workspace --all-targets                                  clean
cargo run -p aex-workspace-check
    134 member(s) and 141 package(s) satisfy every structural and registry rule
cargo run -p aex-workspace-check -- registry build      no change to either file
git diff --check                                         clean
```

`cargo fmt --all` still fails in this worktree with `os error 206`, so the four
owned packages are formatted individually.

### 17. Wake-reliability-pass gate output

```text
cargo fmt -p aex-brain-domain -p aex-brain-application \
          -p aex-brain-store-aws -p aex-work-dynamodb                  clean
cargo clippy -p aex-brain-domain -p aex-brain-application \
             -p aex-brain-store-aws -p aex-work-dynamodb \
             -p brain-mux --all-targets -- -D warnings                clean
cargo nextest run -p aex-brain-domain -p aex-brain-application \
                  -p aex-brain-store-aws -p aex-work-dynamodb \
                  -p brain-mux
    Summary [216.550s] 444 tests run: 444 passed, 0 skipped
cargo nextest run -p aex-brain-application --features loom \
                  --test concurrency  (LOOM_MAX_PREEMPTIONS=3)
    Summary [9.681s] 6 tests run: 6 passed, 0 skipped
cargo test -p aex-brain-application activation::tests
    21 passed, 0 failed
cargo check --workspace --all-targets                                  clean
cargo run -p aex-workspace-check
    134 member(s) and 141 package(s) satisfy every structural and registry rule
cargo run -p aex-workspace-check -- registry build      no change to either file
git diff --check                                         clean
```

### 18. Canonical-generation-pass gate output

```text
cargo fmt -p aex-session-dynamodb -p aex-brain-domain \
          -p aex-brain-application -p aex-brain-store-aws \
          -p aex-brain-hands -p aex-brain-test-support -p brain-mux   clean
cargo clippy -p aex-session-dynamodb -p aex-brain-domain \
             -p aex-brain-application -p aex-brain-store-aws \
             -p aex-brain-hands -p aex-brain-test-support \
             -p brain-mux --all-targets -- -D warnings                clean
cargo test -p aex-session-dynamodb -p aex-brain-domain \
           -p aex-brain-application -p aex-brain-store-aws \
           -p aex-brain-hands -p brain-mux
    554 passed, 0 failed
cargo check --workspace --all-targets                                  clean
cargo run -p aex-workspace-check
    134 member(s) and 141 package(s) satisfy every structural and registry rule
git diff --check                                                        clean
```

### 18.1 Canonical session authority at lease claim

Brain lease claim no longer decodes the removed slim `SessionHead` or compares
the removed `SessionLifecycle` enum. After the conditional agent claim it
strongly reads and decodes the complete canonical session document, verifies
the exact claimed session id and checked workspace projection, and derives the
workspace, organization and deletion epoch from that single authority. Every
non-live deletion state is terminal to a claim. An absent parent is terminal as
well: a durable wake or agent row may legitimately outlive a purged session,
and classifying that stale delivery as corrupt storage would retry or poison it
indefinitely. A present malformed row remains an undecodable store failure.

This does not make lease acquisition and the session head one transaction. The
claim's subsequent decision writes and every renewal still condition on the
session lifecycle, cancellation epoch and deletion epoch, so a deletion race
cannot authorize work. A claim that discovers a terminal session may leave its
new agent lease until expiry; removing it would add another conditional write
to a path that will perform no decision, while it cannot bypass the later
session guards. Converting claim to one `TransactWriteItems` would remove that
temporary lease but lose `ReturnValues=ALL_NEW`, forcing another strong agent
read. The current choice keeps the hot successful claim at two round trips and
preserves the returned fence/head atomically with the claim update.

### 19. Capacity-accounting pass gate output

```text
cargo fmt -p aex-brain-application -p brain-mux                      clean
cargo test -p aex-brain-application -p brain-mux
    application unit 101, threaded concurrency 15, ports 6,
    brain-mux 112; 234 passed, 0 failed
LOOM_MAX_PREEMPTIONS=3 cargo test -p aex-brain-application \
    --features loom --test concurrency                               7 passed
cargo clippy -p aex-brain-application -p brain-mux \
    --all-targets --all-features -- -D warnings                      clean
git diff --check                                                     clean
```

The local target directory is on a Windows volume where Cargo incremental
hard-link creation falls back to copying. Those host warnings do not originate
in source and no lint or test was suppressed.
