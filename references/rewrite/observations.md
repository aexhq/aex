---
title: Observations stream handoff — admission, authority, bounded query, export and composition
description: What the observations stream implemented on rw/observations, what it deliberately left as a typed gap, every cross-stream interface it publishes with its exact path, every change it needs from a peer, and every decision it took beyond the orchestrator conventions and plan 11.
keywords:
  - observations
  - otlp
  - dynamodb
  - export
  - query
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-01
related:
  - references/rust-native-rewrite-2026-07-31/plans/00-orchestrator-conventions.md
  - references/rust-native-rewrite-2026-07-31/plans/11-observations.md
  - references/rewrite/contracts.md
  - references/rewrite/test-architecture.md
---

# Observations stream handoff

Branch `rw/observations`. Everything below is on that branch and nothing is
pushed.

**ClickHouse and Kinesis do not appear anywhere in this work** — not as a
dependency, not as a feature flag, not as a compatibility path, not as a
deferred migration. DynamoDB plus S3 are the authority *and* the query engine.

## 1. What is implemented

### `crates/aex-observation-domain`

The pure authority model, with no AWS, HTTP or OTel SDK anywhere in its graph.

| Module | What it owns |
| --- | --- |
| `canonical` | `CanonicalValue`, RFC 8785 JCS bytes, the batch intent digest over `{principalId, method, canonicalRoute, workspaceId, scope, observations}`, and the attribute digest |
| `keys` | Every `observation-authority` key template, the validated `KeyComponent`, `BucketHour`, `ScopeKey`, the closed `ControlDomain` and `IdempotencyScope` vocabularies, and `parse_observation_pk` |
| `order` | The immutable ordering tuple `(time-or-accepted, signalRank, observationId, revision)` |
| `signal` | The five signals as a bitset; `Signal::in_observation_authority` is `false` for `events` only |
| `batch` | The admission receipt state machine: `preparing → committed | aborted`, resume-by-digest, no transition out of a terminal state |
| `frontier` | The accepted frontier (contiguous advance only) and the `ScopeDeletion` fence with its monotone epoch |
| `gap` | Immutable revisioned gaps, `TimeWindow`, `OrdinalRange`, and `PRODUCIBLE_REASONS` |
| `series` | `SeriesHash` over the pinned input set, and `SeriesClaims` — the sequential model the store's sharded conditional `ADD` must reproduce |
| `limits` | Every pinned ceiling in one module, so a second contradictory copy cannot appear in an adapter |

`replay_expired` stays in the wire enum and is proven unreachable three ways:
it is absent from `PRODUCIBLE_REASONS`, `GapRevision::try_open` refuses it
outright, and no arm of `use_cases::reason_for` can produce it. Three tests
assert this.

### `crates/aex-otlp-admission`

- `proto/` vendors the `opentelemetry-proto` tree; `build.rs` compiles it with
  `prost-build` into a nested `include_file`. **No `tonic`, no `opentelemetry`
  SDK crate.** A test reads the generated output and asserts no gRPC stub exists.
- `proto_pin` digests the vendored `.proto` sources (LF-normalized,
  path-length-prefixed) against `proto/VENDORED.sha256`.
- `decode` — encoded-size gate before any allocation, per-chunk gzip inflation
  under both the decoded ceiling and the ratio guard, record-count ceiling.
- `json` — a hand-written protobuf-JSON decoder over the *same* generated
  structs: 64-bit integers as number or decimal string, `traceId`/`spanId` as
  exact-width lowercase hex, enums by name or number, both `lowerCamelCase` and
  the proto field name, and **unknown fields rejected**. Every metric shape
  (gauge, sum, histogram, exponential histogram, summary) decodes.
- `memory` — an atomic decode-memory budget with `Drop`-released leases.
- `normalize` — identity overwrite, the reserved `aex.*` policy (`aex.internal.*`
  rejects the whole batch), per-signal normalization, and per-record bounds that
  report the exact record pointer.
- `redact` — `DigestRedactor`, a keyed HMAC sliding-window matcher over a
  `{len, hmac}` manifest, with **zero decrypt permission**; budget exhaustion
  fails the batch closed.
- `response` — `partial_success` is the empty message on every success path.
- `limits::OtlpLimits::REGISTERED` pins **4 MiB encoded**. The replaced
  implementation's 6 MiB is not ported, and a test asserts the difference.

### `crates/aex-observation-store-aws`

- `expressions` — the slim 25-field dense index projection (`attrS`/`attrN`/
  `attrB` deliberately absent), the seven indexes and their key attributes, and a
  placeholder-only `ExpressionBuilder`. Nothing customer-supplied ever reaches an
  expression string.
- `segments` — the per-`(scope, signal, bucket)` directory that structurally
  replaces the removed projection: exact fan-out, free empty hours, and a shard
  count that is fixed for a bucket's life.
- `spool` — the `{outbox, index, wake}` pending set, exponential backoff to a
  15-minute ceiling, escalation at 12 attempts, and the ingress-gate evaluator
  with its one-way recovery through `degraded` and its hysteresis dwell.
- `store` — `AdmissionPlan`, the transaction P and C envelopes, the item-size
  model, and `StoreError` including `CommitAmbiguous` and `EnvelopeExceeded`.
- `composition` — the per-role capability grant.
- `health` — `/internal/healthz`, `/internal/readyz` and the probe set.

### `crates/aex-observation-query`

- `ast` — the closed field policy over `COMMON_FIELDS` plus the per-signal sets
  plus `attributes.<key>`; protected fields refused **by name**; a missing field
  makes an ordinary comparison false and `exists` is the only way to observe
  absence; a residual-aware predicate evaluator.
- `plan` — a total function to exactly one access pattern per signal. **No
  `Scan` call exists anywhere in the crate.**
- `plan::classify` — the budget outcome: a short page **with a cursor** for every
  exhausted dimension, and `NoProgress` (the typed `409`) only when a first
  segment yields nothing at all.
- `coverage` — `Snapshot::pin` subtracting `AEX_OBS_INDEX_SETTLE_MS`, honest
  `caught_up`, and `earliest_replay` as the beginning of retained data.
- `cursor` — the twelve-field `ObservationCursorBinding` with an exhaustive
  mismatch check, the 24-hour boundary, the deletion-epoch check, and the
  position-count bound.
- `aggregate` — a bounded streaming pass with reset-aware `increase`/`rate`, a
  weight-carrying t-digest at the pinned compression, and pre-read rejection of
  invalid instrument/calculation pairs.

### `crates/aex-observation-application`

`SemanticEventSource`, `SecretManifestSource` and `ObservationAuthority` ports,
and `AdmitBatch::admit_semantic` with the `GapOnFailure` rule. A producer that
elected `Open` records a durable gap and completes; an unrecordable gap is
**never** downgraded to success.

### `crates/aex-observation-export`

Pinned gzip and ZIP settings with a test that a changed pin changes the hash; a
checkpointing NDJSON encoder where every record boundary is a safe checkpoint;
part-listing verification that treats a truncated listing as a hard integrity
failure; and publication fenced on state, lease, cancel and deletion epoch, where
losing the fence exits **zero**.

### The five deployables

Each declares `ROLE` and `REQUIRED_PROBES` and refuses to start when it observes
a capability outside its grant or when a declared probe has not passed. The
launcher's grant is exactly `LaunchExportTasks` — no observation read of any
kind — and only the `deletion.execute` reconciler deployment may delete an
object.

## 2. G7 — the `PERF-08` staged-commit proof, and the protocol change it forced

`crates/aex-observation-store-aws/tests/g7_staged_commit_envelope.rs`.

**G7 failed on first run and the protocol changed, not the public limit.** A
staged page bounded only by `AEX_OBS_PAGE_RECORDS = 100` produced an 819 KiB
item at the maximum 2,000-point / 16 MiB batch — three times DynamoDB's 256 KiB
item ceiling. A staged page is now bounded by **both** a record count and
`AEX_OBS_PAGE_MAX_BYTES = 192 KiB`, which is three times the 64 KiB
per-observation ceiling, so a page always holds at least one observation however
large it is. The public 2,000-point limit is unchanged.

Measured result, at every signal fan-out the protocol permits:

```text
G7 fan-out=1 records=2000 pages=87 P(actions=2, bytes=1150) C(actions=8,  bytes=10360) materialization_batches=80
G7 fan-out=2 records=2000 pages=87 P(actions=2, bytes=1150) C(actions=11, bytes=11896) materialization_batches=80
G7 fan-out=3 records=2000 pages=87 P(actions=2, bytes=1150) C(actions=14, bytes=13432) materialization_batches=80
G7 fan-out=4 records=2000 pages=87 P(actions=2, bytes=1150) C(actions=17, bytes=14968) materialization_batches=80
```

Against the 100-action / 4 MiB envelope and the designed 24-action ceiling for
transaction C, that is 4.9x to 12.5x action headroom and roughly 280x byte
headroom. The commit's action count is **independent of the record count**,
which is the property that makes the whole staged protocol work, and a separate
test asserts it directly.

The action-count model is exact (one receipt flip, three actions per signal, one
spool chunk, one usage outbox item, one quota finalization, one series-counter
shard). The byte model is deliberately conservative: 512 bytes per control row
against rows that are all well under that, so a model that errs in the safe
direction still proves the envelope. Replacing the model with a live
`TransactWriteItems` measurement is a `tests/live/` item.

## 3. Query shapes supported versus typed-unsupported

| Shape | Status |
| --- | --- |
| session scope, `order.by:"time"` | supported, `gsi_scope_time` |
| session scope, `order.by:"accepted"` | supported, base table, **strongly consistent** |
| workspace scope, either order | supported, `gsi_ws_time` / `gsi_ws_accepted` |
| several signals merged | supported, one walk per signal merged on the ordering tuple |
| `where traceId = X` and `traces/{traceId}` | supported, `gsi_trace` |
| exact-name metric aggregate | supported, `gsi_metric`, bounded at `metric.aggregate_scan` |
| gaps at workspace or session scope | supported, `gsi_gap` / base table |
| `events` at either scope | supported **through the port**; the workspace axis needs the peer GSI in §5 |
| filters over indexed common and signal fields | supported as an index-side filter |
| filters over `attributes.*` and log bodies | supported but **budget-bounded**: short pages with cursors; `telemetry_query_budget_exhausted` only when a first segment cannot progress |
| `group by` over high-cardinality attributes | supported, bounded by the same budget plus the 10,000-row cap |
| Parquet export members | **typed-unsupported**: `EncodeError::Unsupported`, see §4 |
| pre-aggregated rollups and materialized views | **absent by design** — each is a projection with a materializer, write amplification and a rebuild duty |
| arbitrary SQL, arbitrary sort, regex | never offered; no change |

## 4. Deliberate typed gaps

Each is a typed error or an absent-and-declared capability, never a `todo!()`, a
silent no-op or a stub that returns 503 unconditionally.

| Gap | Shape | Blocked on |
| --- | --- | --- |
| Parquet export members | `EncodeError::Unsupported { format: "parquet", .. }` | A writer that can be pinned to byte-identical output. Every writer evaluated embeds a creator string carrying its own version, which would make `manifestHash` a false claim. NDJSON and `otlp_json` are complete. |
| HTTP route mounting for both services | The route table, config, capability grant and readiness are implemented; the axum/`lambda_http` binding is not | `aex-regional-http` is a 21-line stub owned by the regional-services stream. Mounting against a stub would produce exactly the unavailable-port stubs this plan removes. |
| Live AWS evidence | Every `tests/live/**` package compiles; none can execute | No credentials, table, bucket, KMS key, ECS cluster or task definition exists (OD-07). Handed over, never skipped. |
| The reconciler's duty bodies | The duty vocabulary, spool state machine, gate evaluator and escalation rule are implemented and tested; the DynamoDB due-scan loop is not | The same `aex-regional-http`/adapter-client seam, plus a live table |
| `AEX_OBS_INDEX_SETTLE_MS = 2000` | A conservative estimate, not a measurement | `aex-live-regional-observation-api` must measure the p99.99 GSI propagation distribution per region and re-pin it |
| `metric.aggregate_scan = 2_000_000` | An unmeasured cap on the one genuinely reduced capability | A dashboard-shaped workload measured against the API deadline |
| `AEX_OBS_PAGE_MAX_BYTES = 192 KiB` | Derived from the item ceiling, not benchmarked | The 4/8/16/32 KiB placement benchmark shared with the content adapter stream |

## 5. Cross-stream interfaces published

```rust
// aex-observation-store-aws
aex_observation_store_aws::keys::parse_observation_pk(&str) -> Option<ObservationWakeKey>;
aex_observation_store_aws::keys::STREAM_VIEW_TYPE;                 // "KEYS_ONLY"
aex_observation_store_aws::composition::{Role, Capability, assert_grant};
aex_observation_store_aws::health::{HEALTHZ, READYZ, Probe, readiness};
aex_observation_store_aws::segments::{Segment, SegmentDirectory};
aex_observation_store_aws::store::{AdmissionPlan, TransactionEnvelope, StoreError};

// aex-observation-application
aex_observation_application::ports::SemanticEventSource;
aex_observation_application::ports::SecretManifestSource;
aex_observation_application::ports::ObservationAuthority;
aex_observation_application::use_cases::{AdmitBatch, GapOnFailure, SemanticAdmission};

// aex-observation-query
aex_observation_query::plan::{plan, NormalizedQuery, Plan, Budget, classify, PageOutcome};
aex_observation_query::coverage::{Snapshot, Coverage, Consistency};
aex_observation_query::cursor::{ObservationCursorBinding, TraceRevisionMode};

// aex-otlp-admission
aex_otlp_admission::{decode, OtlpLimits, MemoryBudget, normalize, ManagedSecretRedactor};
aex_otlp_admission::wire_pending::PendingErrorCode;
```

### The DynamoDB Streams wake contract for `regional-stream`

1. `observation-authority` has `StreamEnabled = true`,
   `StreamViewType = KEYS_ONLY`. There is no event-source mapping, no Pipe and
   no consumer other than `regional-stream`'s in-process reader.
2. Every wake-relevant record's `pk` begins with the literal `OBS#`.
   `keys::parse_observation_pk` returns `Some` for exactly those and `None` for
   every other family — **including a well-formed key naming `events`**, which
   can never legitimately exist in this table. That function is the whole
   filter; `regional-stream` needs no schema knowledge.
3. **A wake is a hint and carries no payload.** `KEYS_ONLY` makes RS-06
   structural: there are no bytes to emit even by mistake.
4. A missed, duplicated or reordered wake changes **latency only**, because the
   tail read is a strongly consistent base-table *range* read from the socket's
   own durable position, never a key read derived from the wake record.
5. `regional-stream` needs no write permission and holds none:
   `Role::Stream.granted()` is exactly `[ReadAuthority]`, asserted by a test.
6. Handoff: replay with the pinned snapshot, then tail from the last replayed
   accepted position. The overlap is bounded by `AEX_OBS_INDEX_SETTLE_MS` and is
   de-duplicated by `(observationId, revision)`.

## 6. Changes needed from peers

| Peer | What is needed |
| --- | --- |
| contracts | Register three error codes now typed in `aex_otlp_admission::wire_pending::PendingErrorCode`: `unsupported_media_type` (415, non-retryable), `telemetry_query_budget_exhausted` (409, non-retryable, `ErrorDetails` carrying `dimension`, `limit`, `observed`, `remedy`) and `export_capacity` (503, retryable). A test asserts none of them collides with a registered code today. |
| contracts | `ObservationCoverage`'s four watermarks are `DecimalU128` epoch-millisecond accepted-time positions. `snapshot` already is; `accepted`, `indexed` and `earliestReplay` are `ObservationWatermark`/`Timestamp` on the generated wire and need the same scalar treatment, or this stream must lose information at the boundary. |
| contracts | `ExportManifest` / `ExportMember` as a published schema under `api/schemas/`. The Rust shape is in `aex_observation_export::manifest`. |
| regional stores | (a) `session_event.eventId` must be an `ObservationId`; (b) `occurredAt` monotone with `eventSeq`; (c) one sparse workspace-axis GSI on `session-authority` (`evPk = "EVTW#{workspace_id}#{tbucket}"`, `evSk = "{occurred_at}#{session_id}#{event_id}"`); (d) the `observation-authority` entry accepted into `migrations/regional/generated/regional-tables.json`. Without (c) the workspace-axis `events` query is unimplementable, and the fallback is a peer transaction change either way. |
| regional secrets | `regional-secret-custody` must publish a `REDACT#{session_id}` manifest of `{len, hmac}` under a regional redaction key, readable by the `regional-otlp` role **with no decrypt permission**. `ManagedSecretRedactor` is implemented against it today with a real test implementation, never a `todo!()` and never a silent no-op. |
| regional services | `aex-regional-http` must expose `cursor::{encode, decode}` with an extensible binding payload so `ObservationCursorBinding` uses the one codec (RS-17), `EdgeStack` for route mounting, and `capability::Grant<C>` constructors. This is the single blocker on both services' HTTP surface. |
| usage | The `storage.byte_min.v1` fact field set and the regional queue identity the `SPOOL#…/OUTBOX#` item delivers to. This stream writes the fact and never rates it. |
| delivery / infra | The observation bucket policy, the `exports/*` 25-hour backstop rule, the export ECS task definition and its execution role, and the EventBridge schedule per reconciler duty. |
| test architecture | The five deployables' `package.metadata.aex.owner` still reads `regional-services`; plan 11 assigns them to this stream. Left unchanged to avoid a registry conflict — reassign deliberately. |

## 7. Decisions taken beyond plan 11 and the orchestrator conventions

| # | Decision | Rationale |
| --- | --- | --- |
| OB-01 | A staged page is bounded by **bytes as well as records**: `AEX_OBS_PAGE_MAX_BYTES = 192 KiB` alongside `AEX_OBS_PAGE_RECORDS = 100` | G7 measured that a count-only bound produces an 819 KiB item at the maximum batch, three times the provider ceiling. The rule was "change the protocol, never silently cap", and this is that change. |
| OB-02 | The `proto` pin digests the vendored `.proto` **sources**, not `prost-build`'s output | The output's formatting moves with a code-generator patch release, which would fail the pin for a reason that is not a protocol change. The risk the pin exists to catch — an edited or partially synced vendor tree — is fully covered by digesting the sources. |
| OB-03 | Error codes the generated wire does not carry live in a typed `PendingErrorCode` enum rather than being approximated onto a nearby registered code | An approximation is invisible; a typed pending code is a single delete when contracts catches up, and a test proves it does not collide with a registered spelling today. |
| OB-04 | The transaction envelope is proven from an explicit, conservative item-size **model** rather than a live measurement | The SDK exposes no serialized size, and no live table exists (OD-07). A model that errs in the safe direction still proves the envelope; the live measurement is a `tests/live/` item. |
| OB-05 | The accepted "34-name field allowlist" is recorded as what it measurably is: **36 `(signal, field)` pairs over 28 distinct names** | `traceId`, `spanId`, `name`, `durationNs`, `serviceName` and `revision` appear on more than one signal. Both counts are asserted rather than one being quietly restated as the other. |
| OB-06 | The t-digest carries **weights**, not bare values | An unweighted merge shifts the distribution: the first implementation returned 9001 as the median of 1..10000. Weighted centroids make the quantile a real approximation rather than a plausible-looking number. |
| OB-07 | The decode-memory gate is an atomic counter, not a `tokio` semaphore | It keeps the decoder runtime-free and makes the `Drop`-release leak property testable synchronously. The 50 ms reservation *wait* is the service's job. |
| OB-08 | A redaction hit replaces the **whole value**, not just the matched window | Cutting a secret out of surrounding text leaks its position and its exact length, which is most of what an attacker needs. |
| OB-09 | Parquet is refused with a typed error rather than produced non-deterministically | A "deterministic hash" over a non-deterministic encoder is a false claim, and the manifest hash is the only thing that makes an export verifiable. |
| OB-10 | The fuzz property runs as a `proptest` in the **default** lane, not only in a separate fuzz job | A fuzz target nothing gates on is not a gate. The decoder is an untrusted parser and its no-panic property blocks every merge. |

## 8. Gate output

All five gate commands pass on `rw/observations`.

```text
cargo fmt --all                                              clean
cargo clippy -p <11 owned packages> --all-targets -- -D warnings
                                                             clean, zero warnings
cargo nextest run -p <11 owned packages>
    Summary [ 293.246s] 265 tests run: 265 passed, 0 skipped
cargo check --workspace --all-targets                        Finished in 8m 51s
cargo run -p aex-workspace-check
    aex-workspace-check: 133 member(s) and 139 package(s) satisfy every
    structural and registry rule
    aex-workspace-check: 574 unearned-evidence row(s) recorded in the
    source-rewrite phase
```

No `#[ignore]`, no environment-variable self-skip, no empty suite and no
retry-to-green anywhere in the stream; `cargo nextest` reports `0 skipped`.
