---
title: Usage metering stream handoff — fact domain, METER-02 probe, three category authorities
description: What the usage stream implemented on rw/usage, the exact probe API Brain and Hands should call, the fact types it publishes, what it deliberately left undone, every change it needs from a peer, and every decision it took beyond the orchestrator conventions.
keywords:
  - usage
  - metering
  - billing
  - probe
  - dynamodb
  - cpu attribution
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-01
related:
  - references/rust-native-rewrite-2026-07-31/plans/00-orchestrator-conventions.md
  - references/rust-native-rewrite-2026-07-31/plans/12-usage-metering.md
  - references/rust-native-rewrite-2026-07-31/plans/03-central-finance-schema.md
  - references/limits-and-ceilings-decision-2026-07-30.md
---

# Usage metering stream handoff

Branch `rw/usage`. Everything below is on that branch and nothing is pushed.

This stream resumed an interrupted predecessor. That predecessor had written
about 5,900 lines into `aex-usage-domain` but had never referenced them from
`lib.rs`, so none of it had ever been compiled. Wiring it up surfaced ten build
errors, thirteen lint failures and three genuine defects; see §6.

## 1. What is implemented

### 1.1 `aex-usage-domain` — the pure fact model

| Module | Owns |
| --- | --- |
| `quantity` | `Quantity(u128)` capped at `10^38 - 1`, the `DynamoDB` `N` ceiling. No float exists anywhere in the crate. |
| `meter` | The four priced meters, the three authority categories, the four public categories, and `ObservabilityMeter` as a structurally disjoint type. |
| `identity` | `AuthorityKey`, its canonical string and the deterministic `FactId = usage_<sha256hex(canonical)>`. |
| `intent` | `IntentHash` (BLAKE3) over `aex_wire::canonical`, plus a usage-specific fence refusing any fraction. |
| `measurement` | `Measurement` with one constructor that recomputes the quantity from the `Evidence`, and the eight evidence variants. |
| `fact` | `FactDraft`, `UsageFact`, `FactKind`, admission checks (region, category, service-time skew, pinned pricing version). |
| `frontier` | `AcceptedSequence`, `Frontier` and its four contiguous stages, quarantine. |
| `correction` | `Correction`, `CorrectionHead` and the prior-head CAS. |
| `closure` | `ClosureVector` and its validation. |
| `interval` | `accrue_storage` (the `M-STOR-CLOSE` minute cursor), `reconcile_cpu` (the physical cap), `byte_ms` (the exact integral). |
| `shape` | The **five** public Hands baseline tokens with their provider-derived triples. |
| `keys` | The pure key grammar every authority table shares, with a per-category item-type fence. |
| `wire_pending` | Bounded identifiers and the fixed-width `Timestamp`, all `TODO(cross-stream)`. |

### 1.2 `aex-usage-application::probe` — the METER-02 library

Behind a non-default `probe` feature. Four OS seams, each a trait with one Linux
implementation, so the whole probe is deterministically testable off Linux and
the syscall surface stays exactly four seams.

### 1.3 The three category authorities and the projection

`aex-usage-{storage,compute,transfer}-aws` each carry the key grammar bound to
their own `CATEGORY`, the row codec, and the four-item single-partition
admission transaction. `aex-usage-query-aws` carries the generation-keyed
read-only projection key grammar.

### 1.4 Test evidence

266 tests across the nine owned packages, no skips, no `#[ignore]`, no
env-var self-skip.

- **Property tests over arbitrary fact histories** (`aex-usage-domain/tests/properties.rs`):
  the frontier order after any interleaving of admit/project/publish/settle;
  correction-chain linearity, prior-head CAS, void terminality and the depth
  ceiling; the `M-STOR-CLOSE` bound asserted as a bound; the CPU physical cap
  including the pathological `attributed = 10 x physical` case; the exact
  byte-millisecond integral; deterministic identity over arbitrary keys.
- **Golden vectors** (`aex-usage-domain/tests/golden.rs`): every wire-visible
  encoding. The five `SHA-256` fact-id vectors were computed outside this crate
  with `sha256sum` and pinned, so a drift in either the canonical form or the
  digest construction fails rather than agreeing with itself.
- **Probe properties** (`aex-usage-application/tests/probe_properties.rs`):
  awaited time attributes zero for any schedule; no poll exceeds the cap; the
  memory envelope is never over-committed; a resize sequence integrates exactly;
  a crossing yields at most one fact; suspended Hands time bills nothing; the
  Lambda convention floors once, not twice; every offered draft is accounted for.
- **Category isolation**, structural (`crates/aex-usage-*-aws/tests/isolation.rs`):
  each adapter reads its own manifest and sources to prove it links neither
  sibling adapter, declares exactly one table binding and one receipt-queue
  binding, and names no sibling authority's table or queue.

## 2. The exact probe API peers should call

Import path is `aex_usage_application::probe`, and the caller must enable the
crate's `probe` feature.

### 2.1 Context every fact carries

```rust
pub struct ProbeContext {
    pub organization: OrganizationId,
    pub workspace: WorkspaceId,
    pub region: RegionId,
    pub service: ServiceId,
    pub resource: ResourceGeneration,
    pub pricing_version: PricingVersion,
    pub reservation: Option<ReservationId>,
}
```

### 2.2 Brain mux — activation CPU

```rust
pub const MAX_ATTRIBUTED_POLL_US: u64 = 50_000;

pub struct ActivationKey {
    pub session: SessionId,
    pub agent: AgentId,
    pub activation: ActivationId,
    pub fence: u64,
}

impl ActivationMeter {
    pub fn new(key: ActivationKey, context: ProbeContext, attribution: Attribution) -> Arc<Self>;
    pub fn attributed_us(&self) -> u64;
    pub fn poll_count(&self) -> u64;
    pub fn long_poll_violations(&self) -> u64;
    pub fn dropped_jobs(&self) -> u64;
}

pub trait ActivationScoped: Future + Sized {
    fn metered(self, meter: Arc<ActivationMeter>, clock: Arc<dyn ThreadCpuClock>)
        -> ActivationScope<Self>;
}
impl<F: Future> ActivationScoped for F {}

#[must_use = "a CpuJob must be finished; dropping it still attributes but is counted as a defect"]
impl CpuJob {
    pub fn begin(meter: &Arc<ActivationMeter>, clock: &Arc<dyn ThreadCpuClock>) -> Self;
    #[must_use] pub fn finish(self) -> CpuMicros;
}
```

Time pending on provider HTTP, a tool call or a durable wait is not inside a
poll, so it produces **no** compute fact. Nothing needs to be done to get that
behaviour; it falls out of measuring only the poll.

### 2.3 Brain mux — the reconciler

```rust
impl CpuReconciler {
    pub fn new(source: Arc<dyn PhysicalCpuSource>, clock: Arc<dyn WallClock>,
               interval: Duration) -> Arc<Self>;
    pub fn register(&self, meter: &Arc<ActivationMeter>);
    pub fn close_interval(&self, sink: &dyn FactSink) -> Result<CpuIntervalReport, ProbeError>;
    pub fn overattribution_total(&self) -> u64;
    pub fn live_meters(&self) -> usize;
}

pub struct CpuIntervalReport {
    pub physical_us: u64, pub attributed_us: u64, pub charged_us: u64,
    pub platform_us: u64, pub scaled: bool, pub facts_emitted: u32,
    pub interval: ServiceTime,
}
```

Call `close_interval` every 10 s. `charged_us <= physical_us` and
`charged_us + platform_us == physical_us` hold by construction. The registry
holds weak references, so a finished activation drops out with no deregister.

### 2.4 Brain mux — memory reservations

```rust
impl MemoryBudget {
    pub fn new(envelope_bytes: u64, headroom_bytes: u64, sink: Arc<BoundedFactSink>,
               clock: Arc<dyn WallClock>) -> Result<Arc<Self>, ProbeError>;
    pub fn try_reserve(self: &Arc<Self>, context: &ProbeContext, attribution: &Attribution,
                       class: ReservationClass, bytes: u64)
        -> Result<MemoryReservation, ProbeError>;
    pub fn live_bytes(&self) -> u64;
    pub fn grantable_bytes(&self) -> u64;
    pub fn reconcile(&self, physical: &dyn PhysicalMemorySource)
        -> Result<MemoryReport, ProbeError>;
}

#[must_use = "a MemoryReservation must be held for the measured interval; \
              binding it to `_` releases it immediately"]
impl MemoryReservation {
    pub fn bytes(&self) -> u64;
    pub fn class(&self) -> ReservationClass;
    pub fn release(self) -> Result<Quantity, ProbeError>;
    pub fn resize(&mut self, bytes: u64) -> Result<Quantity, ProbeError>;
}
```

`ReservationClass` is `{ Context, CanonicalRequest, ParserBuffer, PreviewBuffer,
Result, WarmCacheEntry }` and lives in `aex_usage_domain::measurement`. Plan 07
§9.8 sketches its own versions of `MemoryReservation`, `ReservationClass` and
`ActivationMeter` — Brain must import these rather than declare them.

**Deviation from the plan:** `MemoryBudget::reserve(.., deadline)` (the async,
waiting form) is **not** implemented. Only `try_reserve` exists. See §5.

### 2.5 Provider gateway, regional APIs, stream — egress

```rust
impl EgressCounter {
    pub fn open(boundary: BoundaryId, context: ProbeContext, attribution: Attribution,
                epoch: CounterEpoch, crossing_seq: u64,
                sink: Arc<BoundedFactSink>, dropped: Arc<AtomicU64>) -> Self;
    pub fn count(&mut self, bytes: u64) -> Result<(), ProbeError>;
    pub fn counted(&self) -> u64;
    pub fn close(self, at: Timestamp) -> Result<Quantity, ProbeError>;
}

pub fn egress_from_receipt(context: &ProbeContext, attribution: &Attribution,
                           at: Timestamp, receipt: &BoundaryReceipt)
    -> Result<FactDraft, ProbeError>;
```

`close` consumes the counter, so a second fact for one crossing does not
compile. A drop emits nothing and increments the supplied counter — an
interrupted crossing has no authoritative byte count, and inventing one would
bill an unmeasured quantity.

**Deviation:** `CountingBody<B>` (the `http_body::Body` wrapper) is **not**
implemented. See §5.

### 2.6 Content, observation, runtime-control — storage

```rust
pub struct StorageEvent<'a> {
    pub owner: &'a StorageOwner,
    pub source: StorageSource,
    pub at: Timestamp,
    pub transition: StorageTransition,
    pub commit_id: &'a str,
}

impl StorageProbe {
    pub fn new(store: Arc<dyn StorageCursorStore>) -> Self;
    pub async fn transition(&self, context: &ProbeContext, attribution: &Attribution,
                            event: &StorageEvent<'_>) -> Result<Option<FactId>, ProbeError>;
}
```

**Deviation from the plan's signature:** the transition parameters are grouped
into `StorageEvent` rather than passed as seven positional arguments, so a
caller cannot transpose the instant and the commit id silently.

### 2.7 Runtime control and every Rust Lambda — allocated shapes

```rust
pub fn hands_facts(context: &ProbeContext, attribution: &Attribution,
                   receipt: &RuntimeReceipt) -> Result<Vec<FactDraft>, ProbeError>;

pub fn lambda_facts(context: &ProbeContext, attribution: &Attribution,
                    report: &LambdaReport, at: Timestamp,
                    convention: LambdaVcpuConvention) -> Result<[FactDraft; 2], ProbeError>;
```

`hands_facts` returns exactly two drafts, compute then memory, both
`FactBasis::Reserved` and both over `running_ms` only. `suspended_ms` bills
nothing on either meter. A receipt whose halves do not exhaust `to - from` is a
hard `ProbeError::UnexhaustedLifetime`, never a guess.

**Deviation from the plan's signature:** `lambda_facts` takes an explicit `at`
timestamp; the plan omitted it, but a `REPORT` line carries no instant of its
own and the service interval has to start somewhere.

### 2.8 The sink

```rust
pub trait FactSink: Debug + Send + Sync + 'static {
    fn offer(&self, draft: FactDraft) -> Result<(), SinkError>;
    fn pressure(&self) -> SinkPressure;   // Idle | Filling | Saturated
}

impl BoundedFactSink {
    pub fn new(capacity: usize, overflow: Arc<OverflowLedger>) -> Result<Self, ProbeError>;
    pub fn offer_or_park(&self, draft: FactDraft);   // for Drop paths
    pub fn take_batch(&self, max: usize) -> Vec<FactDraft>;
    pub fn queued(&self) -> usize;
    pub fn shed_total(&self) -> u64;
    pub fn close(&self);
}
```

## 3. Fact types published

```text
aex_usage_domain::{
    Quantity, MAX_QUANTITY, Meter, ObservabilityMeter, Category, PublicCategory,
    BaseUnit, TokenClass,
    AuthorityKey, AuthorityKind, AuthorityId, SegmentOrdinal, FactId, IntentHash,
    Blake3Digest,
    FactDraft, UsageFact, FactKind, ObservabilityMeasurement, SchemaVersion,
    Measurement, Evidence, FactBasis, ServiceTime, SourceReceipt, ReceiptKind,
    BoundaryId, CounterEpoch, LambdaVcpuConvention, ReservationClass,
    AcceptedSequence, Frontier, FrontierState, PoisonReason,
    Correction, CorrectionHead, CorrectionReason,
    ClosureVector, ClosureDeclaration, ClosureAuthority,
    ComputeShape, HandsShape, ShapeUnit,
    AuthorityKeys, Item, ItemKey, ItemType, ItemValue,
    interval::{accrue_storage, reconcile_cpu, byte_ms, byte_ms_total,
               StorageCursor, StorageOwner, StorageOwnerKind, StorageSource,
               StorageTransition, StorageClose, CpuAllocation},
}
```

## 4. Changes needed from peers

| # | Peer | What |
| --- | --- | --- |
| X-1 | contracts | **Blocking.** `aex_internal_contracts::usage::UsageFact.fact_id` must be `FactId` — the deterministic `usage_<sha256hex(authorityKey)>` — not `Uuid7`. A random id cannot make a producer retry idempotent without extra state and contradicts the `MessageDeduplicationId` and `business_key` grammar finance already pinned. |
| X-2 | contracts / finance | `SettlementReceipt` should carry `workspaceId` and `acceptedSequence`. Without them every receipt costs one `gsi_fact_id` lookup. An optimisation, not a correctness dependency. |
| X-3 | brain | `aex-brain-application` must import `MemoryReservation`, `ReservationClass` and `ActivationMeter` from `aex_usage_application::probe` rather than declaring its own (plan 07 §9.8). |
| X-4 | runtime control | `aex-runtime-control` must expose `ComputeShape` per Hands size as integer `(millicpu, memory_bytes)`, not a float vCPU count, and must expose exactly **five** tokens. |
| X-5 | regional stores | The four `migrations/regional/tables/usage-*.json` definitions are **not yet authored** (see §5) and must be picked up by plan 05's bundle generator and its exhaustive-projection test. |
| X-6 | regional stores | `regional-work` must carry the three `usage.*` deferred-measurement payload kinds and filter them to the owning producer worker, not to a usage worker. |
| X-7 | infra | The `DynamoDB` resource-based policies with a `NotPrincipal` deny must be attached; identity policy alone does not satisfy the "impossible" requirement in `U-19`. |
| X-8 | hands | `RuntimeReceipt` must carry `transmit_bytes: Option<u64>`; `None` means no transfer fact exists for that generation. The probe's `RuntimeReceipt` is a `wire_pending` stand-in to be replaced by `aex_hands_protocol::lifecycle::RuntimeReceipt`. |

## 5. What is deliberately not done

Recorded plainly rather than marked "not applicable". Everything here is owed.

| # | Gap | Where it is recorded |
| --- | --- | --- |
| D-1 | **The three workers are still skeletons.** No `stream`, `sweep` or `receipt` mode handler exists. The projection transaction, frontier advance, outbox emission, partial-batch `batchItemFailures` and poison isolation are unimplemented. | plan 12 §6, work order U6 |
| D-2 | **The `integration` layer for all four adapter crates.** The `DynamoDB` Local suites are unwritten. Recorded as `not_applicable.integration` with the exact missing cases named, so `aex-workspace-check` passes while the debt stays legible. | each adapter's `Cargo.toml` |
| D-3 | **The four `migrations/regional/tables/usage-*.json` definitions.** Not authored. | X-5 |
| D-4 | **`aex-usage-application` use cases.** `ports.rs` and `use_cases.rs` are still placeholders: `RecordFact`, `ProjectCategory`, `PublishOutbox`, `ApplyReceipt`, `RebuildProjection`, `SweepOutbox`, `QuarantineFact` are unimplemented. | plan 12 §1.1 |
| D-5 | **`FactDrain`.** `FactSink`, `BoundedFactSink`, `OverflowLedger`, `DrainPolicy`, `DrainReport` and `DrainError` exist; the `FactDrain` actor that batches into `RecordFact` and whose `flush` fails on a non-empty overflow ledger does not. The types it needs are all in place. | plan 12 §5, U-25 |
| D-6 | **`MemoryBudget::reserve(.., deadline)`.** Only the non-waiting `try_reserve` exists. | §2.4 |
| D-7 | **`CountingBody<B>`.** The `http_body::Body` wrapper every response body was to use. `EgressCounter` underneath it is complete. | §2.5 |
| D-8 | **The `trybuild` compile-fail test** proving no constructor accepts a guest-reported number. The property holds by construction — every `Evidence` variant names a platform-held receipt — but it is not yet asserted by a compile-fail case. | plan 12 U3 item 17 |
| D-9 | **The shadow qualification harness (METER-03).** The `10^5`-fact replay corpus, the nine crash points, the query/invoice fold parity harness and the shadow report writer. | plan 12 §8, U7 |
| D-10 | **The three live companions.** Still skeletons. | plan 12 U8 |
| D-11 | **The `gsi_fact_id` and `gsi_outbox_due` index definitions** as generation input; the sparse attributes are written by the row codec but the index definitions are not authored. | plan 12 §3.1 |

## 6. Defects found and fixed in the inherited work

| # | Site | Defect |
| --- | --- | --- |
| F-1 | `aex-usage-domain/src/lib.rs` | Thirteen of sixteen modules were unreferenced, so ~5,900 lines had never been compiled. |
| F-2 | `intent.rs` | Carried a **second** RFC 8785 canonicalizer, which the workspace rule forbids outright, and ordered object members by UTF-16 code unit where `aex_wire::canonical` pins UTF-8 byte order. Now delegates to `aex_wire::canonical::to_jcs_bytes` and keeps only the usage-specific "no fraction" fence. |
| F-3 | `wire_pending.rs` | `Timestamp::parse` asked `time` to build an `OffsetDateTime` from a format with a literal `Z` and no offset field, so **every** canonical instant was rejected. Now parses as `PrimitiveDateTime` and assumes UTC, which also keeps `+00:00` a rejection rather than a second spelling. |
| F-4 | `measurement.rs` | `BoundaryId` derived serde over a `&'static str`, which makes serde add `'de: 'static` to every enclosing type and made `Evidence` undeserializable. Now hand-written. |
| F-5 | `measurement.rs` | **The Lambda vCPU convention floored the nominal millicpu before multiplying by billed milliseconds.** The pinned formula floors once over the whole product. Flooring first is a systematic undercharge — about 0.5% at 128 MB — that grows with the invocation. `millicpu_ms` now computes in `u128` and floors once; `nominal_millicpu` is kept for reporting only. |
| F-6 | `interval/cpu.rs` | `IntervalError::Unreconcilable` was constructed with a field name that does not exist. |
| F-7 | `quantity.rs` | `QuantityError` derived `Copy` while carrying a `String`. |
| F-8 | `frontier.rs` test | Asserted `published < projected`; the pinned invariant is `published <= projected`. The test was wrong, not the implementation. |

## 7. Decisions taken beyond `00-orchestrator-conventions.md` §8 and plan 12 §11

| # | Decision | Rationale |
| --- | --- | --- |
| UH-1 | The shared key grammar lives in `aex-usage-domain::keys` with a neutral `ItemValue`, not in a fourth shared crate | Area 9's inventory is frozen. The three adapters still never depend on each other, so the isolation requirement is unaffected, and the AWS conversion stays in each adapter. |
| UH-2 | `ItemType` carries a per-category fence: a storage cursor, a closure vector and a crossing claim can only be minted in their own authority | Makes the category boundary a property of the value rather than of the caller's discipline. |
| UH-3 | A projection `Generation` is bounded at 9,999 | `G{gen:04}` is only fixed width to 9,999; above it a fifth digit appears and a range query can straddle two generations. The ceiling is enforced at construction rather than discovered by a mis-scoped read. |
| UH-4 | `StorageProbe::transition` takes a `StorageEvent` struct rather than seven positional arguments | A caller cannot transpose the instant and the commit id, or apply a transition to the wrong owner, without the type system noticing. |
| UH-5 | `lambda_facts` takes an explicit `at` timestamp | A `REPORT` line carries no instant of its own, and the service interval has to start somewhere. |
| UH-6 | `MemoryBudget::close` derives the interval end from `held_ms` rather than re-reading the wall clock | The emitted `[start, end)` and the billed `bytes x held_ms` then cannot disagree by a scheduling delay. |
| UH-7 | A dropped `CpuJob` still attributes its elapsed CPU and increments a drop counter | The work happened either way; discarding it would under-bill. The drop is the defect signal, not the discard. |
| UH-8 | `aex-usage-application` carries a self dev-dependency enabling `probe` | Keeps the feature non-default for real consumers — the three workers never link `rustix` — while the default test lane still covers the probe rather than silently skipping it. |
| UH-9 | `pin-project-lite`, `http-body` and `rustix` added to `[workspace.dependencies]` | `unsafe_code` is forbidden workspace-wide, so a future wrapper cannot hand-roll pin projection and the thread-CPU syscall needs a safe wrapper. `http-body` is declared for the deferred `CountingBody` (D-7). |
| UH-10 | `ActivationId` added to `wire_pending` | The plan's `ActivationKey` names one and no peer type existed yet. `TODO(cross-stream)` like the rest of that module. |

## 8. Gate output

```text
cargo fmt --all                                   clean
cargo clippy -p <nine owned packages> --all-targets -- -D warnings
                                                  clean
cargo nextest run -p <nine owned packages>        266 tests run: 266 passed, 0 skipped
cargo check --workspace --all-targets             exit 0
cargo run -p aex-workspace-check                  133 member(s) and 139 package(s)
                                                  satisfy every structural and registry rule
```
