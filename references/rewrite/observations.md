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
last_verified: 2026-08-02
related:
  - references/rewrite/contracts.md
  - references/rewrite/test-architecture.md
---

# Observations stream handoff

Plan of record: `references/rust-native-rewrite-2026-07-31/plans/11-observations.md` in the parent workspace.

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
| `gap` | Immutable contiguous revisioned gaps, strict non-empty `TimeWindow`, `OrdinalRange`, scoped `GapRecord` accounting evidence, and `PRODUCIBLE_REASONS` |
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

### `crates/aex-observation-store-dynamodb`

- `expressions` — the bounded 18-field dense index projection (`attrS`/`attrN`/
  `attrB` and internal admission/accounting fields deliberately absent), the seven indexes and their key attributes, and a
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
- `gap` — the one strict `GapRecord` DynamoDB codec and append-only store:
  exact replay is success, unequal replay is conflict, successors must be
  contiguous and preserve evidence, and unknown puts are resolved by a strongly
  consistent exact-key read.
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
- `cursor` — the compact `ObservationResume`: one current bucket, one exact
  state per `(access, signal, shard)`, and the last public ordering tuple.
  Authenticated state is structurally validated, duplicate coordinates fail
  closed, and the worst live 20-segment bucket is tested inside the public
  4096-byte `cur_` ceiling. The older twelve-field binding remains the request
  mismatch/deletion-epoch algebra; signing and snapshot recovery use the one
  regional HTTP codec.
- `aggregate` — a bounded streaming pass with reset-aware `increase`/`rate`, a
  weight-carrying t-digest at the pinned compression, and pre-read rejection of
  invalid instrument/calculation pairs.

### `crates/aex-observation-app`

`SemanticEventSource`, `SecretManifestSource`, `ObservationAuthority` and
`GapSink` ports, and async `AdmitBatch::admit_semantic` with the `GapOnFailure`
rule. A producer that elected `Open` records a complete scoped `GapRecord`
before it completes; an unrecordable gap is **never** downgraded to success.

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
object. **All five now have a real `run()`; see §9.**

## 2. G7 — the `PERF-08` staged-commit proof, and the protocol change it forced

`crates/aex-observation-store-dynamodb/tests/g7_staged_commit_envelope.rs`.

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
| exact-name metric aggregate | supported: workspace scope uses `gsi_metric`; session scope uses its isolated `gsi_scope_time` partitions plus an exact projected `metricName` predicate; both are bounded at `metric.aggregate_scan` |
| gaps at workspace or session scope | supported, `gsi_gap` / base table |
| `events` at either scope | supported through `session-authority`: `gsi_session_events` and `gsi_workspace_events` are hour-partitioned on the same canonical observation tuple while the base journal remains sequence ordered |
| filters over indexed common and signal fields | supported by bounded in-process evaluation over the projected row |
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
// aex-observation-store-dynamodb
aex_observation_store_dynamodb::keys::parse_observation_pk(&str) -> Option<ObservationWakeKey>;
aex_observation_store_dynamodb::keys::STREAM_VIEW_TYPE;                 // "KEYS_ONLY"
aex_observation_store_dynamodb::composition::{Role, Capability, assert_grant};
aex_observation_store_dynamodb::health::{HEALTHZ, READYZ, Probe, readiness};
aex_observation_store_dynamodb::segments::{Segment, SegmentDirectory};
aex_observation_store_dynamodb::store::{AdmissionPlan, TransactionEnvelope, StoreError};

// aex-observation-app
aex_observation_app::ports::SemanticEventSource;
aex_observation_app::ports::SecretManifestSource;
aex_observation_app::ports::ObservationAuthority;
aex_observation_app::use_cases::{AdmitBatch, GapOnFailure, SemanticAdmission};

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

## 9. Composition

Branch `rw/deploy-observations`, off `main` after the four-stream merge. Every
one of the five deployables now has a **real `run()`**: the typed
`NotImplemented` run-error variant is gone from all of them, and none was replaced by a
stub, a `todo!()` or a route that answers `503` because a port was never wired.

Nothing here is deployed, credentialed or published. No AWS call was made and no
`.env*` file was read.

### 9.1 What each deployable became

| Deployable | Host | What `run()` now does |
| --- | --- | --- |
| `regional-observation-api` | Rust Lambda ZIP, `axum` + `lambda_http` | Owns the finite routes in `RouteGroup::Observations` and `RouteGroup::TelemetryLifecycle`, narrows them through one reviewed served predicate, dispatches through the generated `dispatch_observations` / `dispatch_telemetry_lifecycle`, and serves them over a bounded `DynamoDB`/`S3` reader: frontier read, snapshot pin, per-index segment walk, residual predicate evaluation, budget classification, signed cursor, gap reads, export read, revoke and download grant. The two export-admission routes are contained in §12. |
| `regional-otlp` | Rust Lambda ZIP, `axum` + `lambda_http` | Mounts `RouteGroup::Otlp` (3), reserves the worst-case decoded footprint **before the first decode byte**, decodes and normalizes under the reservation, redacts against the keyed digest manifest, then runs the whole staged admission protocol: ingress gate, deletion fence, frontier allocation, transaction P, staging, transaction C, replayable materialization. |
| `observation-reconciler` | scheduled Rust Lambda, `lambda_runtime` | One duty per deployment, selected by `AEX_OBS_DUTY` from the closed `ControlDomain` vocabulary. Due-scans the sparse `gsi_control` index, takes a durable per-item claim, runs the duty body, and answers with a partial-batch failure body rather than throwing. |
| `observation-export-launcher` | Rust Lambda, `lambda_runtime` | Due-scans `export.launch`, takes the fenced lease under an `admitted`-or-`launching` state with an expired lease and no cancellation, `RunTask`s with `clientToken = startedBy = export_id`, and reconciles every ambiguous outcome through `ListTasks{startedBy}` — never through a second `RunTask`. |
| `observation-export-task` | one-shot Rust Fargate task | Takes the lease before any read, acquires every memory reservation before the producing loop starts, streams bounded pages into a checkpointed NDJSON member, uploads parts with a fenced checkpoint after each, verifies `ListParts` to exhaustion on resume, and publishes under one conditional update — losing which aborts the upload and exits `0`. |

`/internal/healthz` and `/internal/readyz` are served by all five, always through
`aex_observation_store_dynamodb::health::{HEALTHZ, READYZ}` rather than a hand-typed
path. `readyz` answers `200` only once every declared probe has actually passed;
an unproven probe is never assumed.

### 9.2 Environment variables

No variable naming a table, bucket, queue, cluster, ARN, key or region has a
default anywhere. Start-up fails fast naming the first variable that is missing
or invalid, and the refusal also prints the whole required list.

`regional-observation-api`, fifteen:

```text
AEX_PLANE                       dev | prd
AEX_REGION                      a regional-plane region name
AEX_OBSERVATION_TABLE           the observation-authority table
AEX_OBSERVATION_BUCKET          the regional observation bucket
AEX_SESSION_TABLE               session-authority, read-only, for events
AEX_OBS_INDEX_SETTLE_MS         2000; must dominate the 1000 ms clock-skew bound
AEX_OBS_QUERY_SCANNED_ITEMS     at most 50000
AEX_OBS_QUERY_SEGMENTS          at most 64
AEX_OBS_QUERY_READ_BYTES        at most 33554432
AEX_OBS_METRIC_AGGREGATE_SCAN   at most 2000000
AEX_CURSOR_SIGNING_KEY_REF      Parameter Store name of the versioned cursor ring
AEX_AUTHZ_FUNCTION_ARN          qualified central-authz Lambda ARN
AEX_AUTHZ_VERIFY_KEYS_PARAM     Parameter Store name of the assertion trust anchors
AEX_AUTHZ_PROJECTION_TABLE      regional-authz-projection, read on every request
AEX_ASSERTION_CACHE_BYTES       assertion cache budget, above zero
```

The finite Lambda resolves both key documents once before its runtime starts;
no signing material appears in its environment or Terraform state. The export
cluster belongs to `observation-export-launcher`, not the API: admission writes
the durable export control row and the launcher supplies its own cluster at the
execution boundary.

The API's `PutItem` and `UpdateItem` statement is separate from its read
statement and is constrained by
`ForAllValues:StringLike { dynamodb:LeadingKeys = ["EXPORT#*"] }`. The Rust
`WriteExportControl` capability is therefore enforced by the deployed IAM
request condition as well as by the application adapter; it cannot mutate
admission, frontier, segment, gap, claim, or deletion rows.

`regional-otlp`, fifteen:

```text
AEX_PLANE                       dev | prd
AEX_REGION                      a regional-plane region name
AEX_OBSERVATION_TABLE           the observation-authority table
AEX_OBSERVATION_BUCKET          the regional observation bucket
AEX_SECRET_CUSTODY_TABLE        regional-secret-custody, for REDACT# manifests
AEX_OBS_REDACTION_KEY_REF       Secrets Manager id of the regional redaction key
AEX_OTLP_ENCODED_MAX            at most 4194304 (4 MiB); 6 MiB is refused
AEX_OTLP_DECODED_MAX            at most 16777216 (16 MiB)
AEX_OTLP_MAX_RECORDS            at most 2000
AEX_OTLP_MEMORY_BUDGET_BYTES    at least AEX_OTLP_DECODED_MAX
AEX_OTLP_RESERVE_WAIT_MS        50
AEX_OTLP_RESERVED_CONCURRENCY   the deployed reservation, above zero
AEX_CENTRAL_AUTHZ_URL           https:// endpoint of the assertion exchange
AEX_ASSERTION_TRUST_ANCHORS     kid:base64-Ed25519-public-key, comma separated
AEX_ASSERTION_CACHE_BYTES       assertion cache budget, above zero
```

`observation-reconciler`, nine:

```text
AEX_PLANE                       dev | prd
AEX_REGION                      a regional-plane region name
AEX_OBSERVATION_TABLE           the observation-authority table
AEX_OBSERVATION_BUCKET          the regional observation bucket
AEX_OBS_DUTY                    one ControlDomain; export.launch is refused
AEX_OBS_RECONCILE_PAGE          1..=1000
AEX_OBS_DUTY_SHARDS             1..=64
AEX_OBS_MAX_ATTEMPTS            1..=12
AEX_USAGE_QUEUE_URL             https:// SQS queue the storage fact is delivered to
```

`observation-export-launcher`, ten:

```text
AEX_PLANE                       dev | prd
AEX_REGION                      a regional-plane region name
AEX_OBSERVATION_TABLE           EXPORT# and CTRL# rows only
AEX_EXPORT_CLUSTER              ECS cluster ARN; its region must equal AEX_REGION
AEX_EXPORT_TASK_DEFINITION      task-definition ARN; same region check
AEX_EXPORT_SUBNETS              comma-separated subnet ids, at least one
AEX_EXPORT_SECURITY_GROUPS      comma-separated security-group ids, at least one
AEX_EXPORT_MAX_CONCURRENT       1..=100
AEX_EXPORT_LAUNCH_SHARDS        1..=64
AEX_EXPORT_LEASE_MS             1000..=900000
```

`observation-export-task`, eleven:

```text
AEX_PLANE                       dev | prd
AEX_REGION                      a regional-plane region name
AEX_EXPORT_ID                   an exp_ identifier
AEX_WORKSPACE_ID                a wsp_ identifier
AEX_OBSERVATION_TABLE           the observation-authority table
AEX_OBSERVATION_BUCKET          the regional observation bucket
AEX_EXPORT_MEMORY_BUDGET_BYTES  at least the sum of the four reservations
AEX_EXPORT_PART_BYTES           5 MiB..=64 MiB; below 5 MiB no upload can complete
AEX_EXPORT_ROWGROUP_BYTES       1 MiB..=256 MiB
AEX_EXPORT_PAGE_LIMIT           1..=1000
AEX_EXPORT_LEASE_MS             1000..=900000
```

Two variables are optional because neither names a resource:
`AEX_RELEASE_DIGEST` (the digest both health endpoints report, default
`unreleased`) on all five, and `AEX_HEALTH_PORT` on the export task, where an
absent or zero port means no listener is bound and the same JSON is served
through `health_body()` and `readiness_body()`.

### 9.3 Where the resource shapes live

The orchestrator asked for the Lambda memory, timeout and reserved concurrency
in `[package.metadata.aex]`. That table's key set is **closed** —
`aex-workspace-check` fails an unknown key — and the shape `graph verify`
actually reads is `release/units.toml`'s `[unit.lambda]` / `[unit.fargate]`
block. All five rows are filled there, each with the reasoning that produced the
number:

| Unit | Shape |
| --- | --- |
| `regional-observation-api` | `memory_mb = 1024`, `timeout_s = 30`, `reserved_concurrency = 40` |
| `regional-otlp` | `memory_mb = 3008`, `timeout_s = 30`, `reserved_concurrency = 20` |
| `observation-reconciler` | `memory_mb = 512`, `timeout_s = 300`, `reserved_concurrency = 8` |
| `observation-export-launcher` | `memory_mb = 512`, `timeout_s = 60`, `reserved_concurrency = 4` |
| `observation-export-task` | `cpu = 2048`, `memory_mb = 4096`, `desired_count = 0`, `stop_timeout_s = 120`, `port = 0` |

`regional-otlp`'s row is the mechanism O-ROLES exists to provide: the
whole-region decode footprint is reserved concurrency times
`AEX_OTLP_DECODED_MAX`, which is 20 x 16 MiB = 320 MiB, so decompression can
never starve query, export or sockets. A test asserts the product stays inside a
stated ceiling, and a second asserts the query role carries its own separate
reservation.

### 9.4 The library change this required

`aex-observation-store-dynamodb` gained `store::pack_pages` and `store::PageSpan`, and
`AdmissionPlan::new` now calls them. The page-packing rule — bounded by both
`OBS_PAGE_RECORDS` and `OBS_PAGE_MAX_BYTES` — previously existed only inside the
planner, so the writer would have had to restate it. There is now exactly one
rule, and the page the G7 envelope was proven over is the page that is actually
written. The 37 existing tests in that crate, G7 included, are unchanged and
still pass.

### 9.5 Decisions taken beyond plan 11

| # | Decision | Rationale |
| --- | --- | --- |
| OB-11 | The authenticated edge is composed in each service from `aex_regional_http::assertion`'s `VerifyingAssertionCache`, with the composition supplying the two things that crate deliberately leaves open: an Ed25519 `KeyVerifier` over trust anchors resolved at start-up, and an `AssertionSource` that exchanges the presented credential at `AEX_CENTRAL_AUTHZ_URL` | The crate's own comment says concrete crypto stays in the composition root, and OD-21 puts the 30-second assertion on Ed25519 because AEX holds that key directly. The exchange body is an internal contract, not a public route; the credential never leaves the regional plane except towards the authority that issued it. |
| OB-12 | The `DynamoDB` item codec for admission and for reading lives in the deployables, not in `aex-observation-store-dynamodb` | The library was explicitly out of scope beyond what mounting requires, and it models the protocol — plans, envelopes, item sizes — rather than executing it. A follow-up may lift the codec into the adapter crate; that is a move, not a rewrite. |
| OB-13 | Predicates are evaluated in process over the decoded row rather than compiled into a `DynamoDB` filter expression | Plan §4.6 compiles indexed predicates index-side to save bandwidth. Doing it in process is equally correct and still bounded, because `max_items_scanned` counts **index items read**, which is exactly the dimension the budget exists to bound. It costs bandwidth, not correctness, and the expression compiler can be added later without changing the public behaviour. |
| OB-14 | The route's signal is the authority over the body's | `POST /api/logs/query` carrying `signal: "metrics"` is a contradiction rather than a preference. The refusal is `invalid_query` naming both spellings. |
| OB-15 | A query operand arrives on the wire as a string and is recovered to its scalar kind before comparison | Comparing `severityNumber > 9` as text makes `10` false. The recovery is total and tested. |
| OB-16 | `telemetry_query_budget_exhausted` and `export_capacity` are reported under the nearest **registered** code with the pending spelling named in the message | Both are still owed by the contracts stream (§6). A typed pending code that names itself is one delete when the vocabulary catches up; silently answering under a code that means something else is invisible. |
| OB-17 | The reconciler and the export task are a library plus a thin binary rather than binary-only | Their required cases — the duty-to-role matrix, partial-batch contents, resume and publication fences — need real calls, which a binary-only crate cannot expose to `tests/*.rs`. This is the shape `services/regional-stream` and `workers/session-operation-worker` already use. |
| OB-18 | `cargo fmt --all` is run in batches of twenty packages | On this Windows host `--all` puts every source path of all 133 members on one command line and fails with `os error 206`. Batching is the same operation with a shorter argv; Linux CI is unaffected. |

### 9.6 What a peer still owes

Everything in §6 still stands. Three items became load-bearing now that the
routes are mounted:

| Peer | What is needed |
| --- | --- |
| central identity/control | The internal assertion exchange this edge consumes: `POST /internal/authz/assertions` taking `{credential, audience, region}` and answering `{assertion, keyId, credentialBinding, signature}`, where `assertion` is `aex_internal_contracts::assertion::AuthorizationAssertion` and the signature is Ed25519 over the credential-bound canonical form `aex_regional_http::assertion::SignedAssertion` already defines. Both services fail closed without it. |
| regional secrets | The `REDACT#{session_id}` manifest item, read here as `pk = "REDACT#{session}"`, `sk = "MANIFEST"`, `custodyRevision` as a number and `entries` as a list of maps carrying `len` and a 32-byte `hmac`. An absent item means no managed secret was injected, which is the only reading that does not invent one. |
| regional services | `aex-regional-http` should absorb the composed edge layer. Each service currently carries its own edge module; the verification algebra is already the crate's, and only the layer that turns a request into a `RequestContext` is duplicated. |

### 9.7 Gate output

```text
cargo fmt (133 packages, batched)                            clean
cargo clippy -p regional-observation-api -p regional-otlp \
             -p observation-reconciler -p observation-export-launcher \
             -p observation-export-task --all-targets -- -D warnings
                                                             clean, zero warnings
cargo nextest run -p <the same five>
    Summary [ 251.432s] 300 tests run: 300 passed, 0 skipped
cargo check --workspace --all-targets                        Finished in 43.08s
cargo run -p aex-workspace-check
    aex-workspace-check: 133 member(s) and 140 package(s) satisfy every
    structural and registry rule
    aex-workspace-check: 515 unearned-evidence row(s) recorded in the
    source-rewrite phase
cargo run -p aex-workspace-check -- registry build           regenerated, committed
cargo run -p aex-release-tool -- graph verify
    1 violation(s): [graph-cycle] cargo:aex-usage-app -> itself
    (pre-existing, another stream's crate; no resource-shape violation on any
    of the five units)
```

No `#[ignore]`, no environment-variable self-skip, no empty suite and no
retry-to-green anywhere in the five packages; `cargo nextest` reports
`0 skipped`. No ClickHouse and no Kinesis appears in any of them, which four
tests assert directly.

### 9.8 Continuation reviewed on 2026-08-02

The pagination and native-event follow-up fixed the bounded-cursor and physical
tuple defects described below. The materialized observation id is deterministic
from the batch, signal and accepted sequence, and a committed receipt now stores
the winning accepted time and per-signal allocation ranges. An equal-intent
client retry therefore rematerializes the same keys rather than reallocating.
Every base and secondary observation sort key uses the same physical spelling as
the public tuple:
`(time-or-accepted, signalRank, observationId, revision)`. Native session and
workspace event indexes use that spelling too; the session journal's base key
stays `eventSeq` ordered for its owning transaction protocol.

Finite pages and replay streams now authenticate the original snapshot and a
bounded per-segment state. Reads are bucket-major, issue the current bucket's
segment queries with at most 16 provider reads in flight, merge one
provider-ordered head per segment, and advance durable state only when a row is
popped. The merge still waits for every live head before selecting a tuple, so
the concurrency cap changes resource pressure rather than ordering. A row
rejected by the snapshot or predicate advances its own segment; a prefetched
but unreturned row does not. A bucket is left only after every segment is
exhausted. Event continuation remains typed: `eventId` is the native event's
`ObservationId`, so its provider key is reconstructed without a lossy id
translation. This removes the old fair-share reread stall and prevents a global
tuple from skipping a later signal or shard.

Every access now participates in the same logical hour groups. The all-time
trace partition is reused once per intersecting hour with an exact half-open
sort range, and the daily metric partition is reused the same way. That closes
the mixed-access failure where a whole trace or metric day could be drained
before an earlier event, log or other hourly segment. Ascending and descending
laws cover trace plus events and metric plus logs across both hour and UTC-day
boundaries. Because the sparse indexes are observation-time ordered, an
accepted-order trace or metric query deliberately uses the accepted-time base
or workspace index instead; every provider segment is therefore monotone in
the tuple being merged.

The same continuation closed four adjacent correctness defects:

- finite page two recovers the authenticated page-one snapshot instead of
  repinning under concurrent ingestion;
- mixed completeness is the minimum selected complete frontier and retained
  replay begins at the maximum selected retained floor; native events inherit
  their authority-time/monotone-sequence invariant and the requested range;
- an explicitly bounded bucket enumeration is half-open and descending reverses
  that bounded walk;
- session metric aggregation never reads the workspace-wide metric partition:
  it uses `gsi_scope_time` plus the exact projected `metricName` predicate.

Focused evidence recorded during the continuation:

```text
cargo test -p aex-regional-http --test primitives cursor       7 passed
cargo test -p aex-observation-query --lib cursor::tests        7 passed
cargo test -p aex-observation-query --lib                     39 passed
cargo test -p regional-observation-api --lib                  46 passed
cargo test -p aex-session-dynamodb --lib <event-key test>      1 passed
cargo test -p regional-otlp --bin regional-otlp               28 passed
cargo test -p aex-observation-store-dynamodb --lib <key test>       1 passed
cargo test -p observation-reconciler --bin observation-reconciler
                                                               compiled clean
cargo clippy -p regional-observation-api -p regional-otlp \
  -p observation-reconciler -p aex-observation-query \
  -p aex-observation-domain -p aex-observation-store-dynamodb \
  -p aex-regional-http -p aex-session-dynamodb --all-targets \
  -- -D warnings                                                clean
cargo run -p aex-regional-test-support --example emit-regional-tables
                                                               13 tables
cargo test -p aex-regional-test-support --lib tables::tests   21 passed
```

The deterministic regional bundle digest after merging current main and
regenerating all concurrent table-definition changes is
`blake3:bc50c6a9339dab0f30c48876d5ed4e68b9322e89c225aab2e0024ac00b9c2fb4`.
Live DynamoDB latency/throttling evidence is still an environment-backed gate,
not a local claim. The reconciler's spool verification remains exact and
bounded but filters `acceptedSeq` within one accepted-hour/shard partition now
that the authority sort key is canonical; a separate verification index is an
optional optimization only if live evidence shows that control-path read cost
is material.

This review does **not** declare admission or earliest replay complete. The
remaining release blockers are exact:

- transaction C does not match `AdmissionPlan::commit_envelope`: it still omits
  the `SEG#` and `SEGT#` updates, quota finalization, series claims/counter, and
  staged-page digests on the committed receipt;
- the authored table omits the segment/control item families it claims to own,
  and the reader does not page the existing segment-directory authority. Its
  epoch-to-now fallback materializes hour descriptors and silently stops at
  `u16::MAX` hours, so `earliest` can omit current data;
- the promised three-actions-per-signal transaction model cannot update every
  `SEGT#` row while one admitted batch may contain observations from arbitrarily
  many event-time hours. Admission must bound that cardinality or move exact
  time-directory publication behind another fenced, completeness-preserving
  protocol before the measured action claim can be true;
- staged pages are retained, but no crash-recovery duty reconstructs and
  materializes a committed batch from them without the original request;

### 9.9 Admission, directory and recovery audit on 2026-08-02

`00a45730` composes the cursor/tuple continuation with main's durable gap
ledger. The composed focused suites are green: `regional-observation-api` has
49 passing library tests (including both the deletion-epoch cursor binding and
gap revision read/stream cases), `regional-otlp` has 31 passing binary tests,
and `aex-regional-test-support` has 21 passing regional-table tests. The table
generator rebuilt 13 tables with the digest above. This is merge and local
structural evidence only; it is not a claim of a DynamoDB transaction or live
AWS proof.

One independently safe receipt-integrity step landed: C now commits the ordered
page digest manifest and both immediate durable-winner consumption and
equal-intent replay verify it before materialization. The remaining
admission/directory/recovery items below deliberately remain blocked. The
accepted plan requires them together, and the current durable facts do not
support a sound partial directory or recovery implementation:

- `AdmissionPlan::commit_envelope` names a receipt, frontier, `SEG#` and
  `SEGT#` updates, spool/outbox, quota finalization and a series-counter shard;
  `regional-otlp` transaction C currently writes only the receipt, per-signal
  frontiers, deletion condition, spool and outbox. C now also commits the
  ordered `pageDigests` calculated from the exact staged bytes, and an
  equal-intent replay recomputes and checks that manifest before it
  rematerializes. P still reserves quota without C finalizing it, and
  `AdmissionPlan::new` is passed zero new-series claims. The plan's measured G7
  number is therefore not an execution proof until one shared transaction-plan
  value drives both the envelope and emitted `TransactWriteItems`.
- The accepted table layout has one flat `SEGT#{scope}#{signal}` row for every
  event-time hour, while an OTLP batch is allowed to contain up to 2,000 records
  with arbitrary event times. Such a batch can require up to 2,000 distinct
  time-directory updates, exceeding DynamoDB's 100-action transaction limit
  and the plan's 24-action C ceiling. There is no public event-time-hour bound
  to enforce, and none may be invented as a hidden admission limit. A correct
  successor needs an accepted publication protocol that makes a bounded,
  paged time-directory manifest visible under the same deletion/receipt fence;
  it must then revise both the G7 model and reader cursor state. It cannot be
  manufactured by separately updating `SEGT#` rows after C, since an
  authoritative reader could then omit already committed observations.
- The reader still derives hourly descriptors from the requested range and
  stops at `u16::MAX`; it does not read `SEG#`/`SEGT#` at all. The table source
  also omits these control item types, so adding a query over a directory that
  C never writes would be a false completion. Removing the cap without the
  directory would replace silent loss with unbounded allocation and provider
  fan-out, which is equally invalid.
- Staged pages currently retain only newline-delimited canonical observation
  bodies. The committed receipt now binds their ordered digests, but neither
  durable form retains the signal, event time, indexed fields, body placement or
  deterministic materialization identity required to rebuild an `OBS#` row.
  `replay_committed` consequently depends on the original in-memory request and
  restages bodies. A reconciler cannot reconstruct a committed batch after that
  process has crashed. A recovery implementation must first define a
  size-accounted, immutable staged-record codec and receipt digest manifest,
  then verify every page before materializing; corruption must produce the
  existing exact `pipeline_loss` gap path, never guessed observations.

The next split is therefore one accepted protocol/design slice, not four local
TODOs: define the visibility-fenced paged time-directory authority, its exact
receipt/staged-record codec, transaction participants and crash recovery
reader; change `AdmissionPlan` to own that full input and update G7 against the
actual action sequence; then make the reader page that authority and remove the
synthetic-hour walk. Until that lands, the current code must not advertise
earliest replay or directory-backed reads as complete.

## 10. Durable telemetry-gap continuation

Continuation branch `rw/continue-regional-gaps`, based on public main at
`cf327b46`, completed the gap path that the original composition left
permissive. Nothing in this section was pushed or deployed.

### 10.1 Canonical record and lifecycle

`aex_observation_domain::gap::GapRecord` is now the one value passed between
producers, persistence and readers. It binds the immutable `GapRevision` to its
owning workspace and exact session-or-workspace scope, plus attempted records,
attempted bytes and recoverability. Empty signal sets and empty time windows are
invalid. Revisions begin at zero and are contiguous; skipping a number is not a
monotone append.

`aex_observation_store_dynamodb::gap::{encode, decode}` is the one strict row codec.
It refuses missing, mistyped or contradictory fields rather than defaulting a
reason, signal, owner, range or repair state. The row is:

```text
pk = GAP#{scopeKey}
sk = {gapId}#{revision:020}
itemType = telemetry_gap
gapId, revision, state, scopeKey, workspaceId, sessionId?
signals, reason, ordinalRange?, timeRange?, unbounded
attemptedRecords?, attemptedBytes?, recoverable
openedAt, revisedAt, repairSource?, repairedAt?
gwPk = GAPW#{workspaceId}
gwSk = {openedAt}#{gapId}#{revision:020}
```

`GapStore::append` treats an identical exact-key replay as success and unequal
evidence as conflict, verifies a non-zero revision's predecessor with a strongly
consistent read, conditionally creates both keys, and resolves an unknown put by
reading the exact key strongly. `append_action` publishes the same codec and
immutability condition to transactions that must terminalize a source atomically
with its gaps.

Every append also advances one counter row per workspace, in its own partition so
that no gap query can decode it as a revision:

```text
pk = GAPV#{workspaceId}
sk = CHANGE
itemType = gap_change_hint
gapAppends
```

It is an `ADD`, so concurrent appends are both counted, and it is the only row a
follow socket needs in order to decide whether reading gap history is worth a
query this cycle. It is never authority: it can run ahead of durable history,
never behind, and a socket that cannot read it, or that sees it move backwards,
reads the ledger. A socket also reads the ledger on its first cycle, on any wake,
and once a minute regardless, so an append whose counter never landed delays a
gap by at most that interval.

### 10.2 Loss production

`regional-otlp` no longer collapses several signal allocations into one lossy
range. Every spool chunk retains a `lossCandidates` list with a source-stable
gap id, one concrete signal, `[acceptedSeqLo, acceptedSeqHiExclusive)`, exact
per-signal record and byte counts, and the smallest half-open observation-time
range that covers that signal. If the exclusive end cannot be represented, the
candidate honestly omits `timeRange` and later becomes unbounded.

Both spool-attempt exhaustion and a proven persistent index hole pass those
candidates through the canonical codec. The reconciler writes every gap revision,
advances the workspace gap-change counter, and updates the source to its terminal
state in one `TransactWriteItems`. The
source update is fenced on key existence and the observed attempt/claim. A
conditional or transport failure is success only when strongly consistent reads
prove the exact terminal source and every exact gap row already exist.

### 10.3 Finite reads, coverage and streams

Session gap history is a strongly consistent base-table query. Workspace history
uses `gsi_gap` only to discover bounded keys, then strongly hydrates complete rows
in batches of 100. Both paths have a 5,000-revision budget, reject malformed
rows through the shared codec, filter revisions above the pinned settled
snapshot, and collapse each gap id to its latest visible revision.

The two gap query routes now apply time, concrete-signal and recoverability
filters, paginate in stable `(openedAt, gapId)` order with a signed request-bound
cursor, and never fabricate an absent `timeRange`. Observation pages, metric
aggregates and trace details compute coverage from the same visible latest gap
set: known holes are clipped to the requested half-open window, unbounded gap
ids are reported separately, and repaired revisions do not make coverage
incomplete.

NDJSON replay/follow producers read that same ledger on every authority pass and
emit `ObservationFrame::Gap` before record/cursor frames. Each connection
deduplicates `(gapId, revision)` while still emitting a later repaired revision,
so clients can observe both incompleteness and its repair without wake hints
being required for correctness.

The public `TelemetryGap.timeRange` contract is now optional, matching the
existing `unboundedGaps` coverage vocabulary. All generated OpenAPI, bundle and
Rust wire artifacts were regenerated from the schema.

### 10.4 Remaining integration evidence

- The durable async `GapSink` is implemented and `AdmitBatch::admit_semantic`
  now persists a complete `GapRecord` before returning `Gapped`. Brain, session
  and Hands producers still own calling this port as already recorded in §6;
  this continuation does not invent cross-repository call sites.
- Repair revisions are fully modeled, stored, read and streamed, but the future
  repair producer must supply its own proven `repairSource`; no repair is
  inferred from an index count.
- DynamoDB-local/live AWS route and ambiguity evidence remains in the selected
  live suites. The default tests cover strict codec round trips and corruption,
  stable replay/successor laws, exact spool candidates, atomic transaction and
  claim-fence shape, coverage semantics, and stream revision deduplication.

## 11. Observation authorization renewal and exact read-byte budget

Branch `rw/observation-auth-budget`, off the integrated public `main`. This pass
closes the two independent blockers removed from §9.8. It does not change or
claim any of the admission, directory or recovery items that remain in §9.9.

### 11.1 A stream cannot exist without a regional authorization lease

`StreamPolicy` has no default and its `revalidator` is no longer optional.
`ObservationService::new` takes the complete policy, so a composition root that
cannot renew authorization cannot construct the service. The producer invokes
the lease before its first authority read and at every authorization interval;
any refusal becomes the terminal typed failure frame.

Both production roots use `RegionalEdge::revalidate`. The finite Lambda now
adapts the same edge it uses for request admission; `regional-stream` retains
its existing adapter and additional session-head check. Renewal keeps only the
verified `RegionalAuthorization`, never credential plaintext, and makes no new
central-plane call. It strongly rereads the projected key floor and workspace
placement, so key/workspace/account epoch advance, organization drift, account
pause, and placement or region change fail the connection closed. Session
deletion remains independently strongly reread by the observation follower.

### 11.2 A refill reserves its whole required-head set before any read

One exact merge step still establishes a head for every live segment before it
selects a tuple. The refill planner now reserves the whole set first. For a
provider page with request limit `L`, the query reservation is
`min(L × 400 KiB, 1 MiB)`: the existing shared DynamoDB item ceiling supplies
the first bound and DynamoDB's Query ceiling supplies the second. A slim index
page that needs strong base-row hydration additionally reserves
`L × 400 KiB`; a dense base-table row is counted only once. The planner chooses
the largest common `L` whose per-segment reservations fit both the remaining
item count and the remaining configured byte budget. If one head per required
segment cannot fit, it issues no provider read and returns the existing typed
no-progress error or a complete short-page cursor.

At most sixteen reserved futures execute at once. They are collected as
independent results rather than short-circuited, so one provider failure does
not cancel siblings that were already admitted. Only after every future settles
does the reader verify each actual query-plus-hydration size against its own
reservation, verify the wave against the remaining ceiling, reconcile unused
bytes, and add actual spend. Segment cursor state still advances only when its
candidate is popped; prefetched but unreturned rows remain unadvanced and are
therefore reread on continuation.

The provider bounds are the published DynamoDB constraints: one item is at
most 400 KiB and one Query evaluates at most 1 MiB. `BatchGetItem` hydration is
also key-count bounded and retries only its unprocessed subset, so at most `L`
distinct base rows are added to the reservation.

## 12. Telemetry-export operation containment

The two telemetry-export admission routes are deliberately absent from the
production router. The dormant handler creates only an `export` row in
`observation-authority`, but returns a generic public `Operation`. Regional
operation GET, list and cancellation are owned by `session-authority`, so that
operation cannot subsequently be read, listed or cancelled there. Reporting
`cancelable: true` does not repair the missing authority. The admission also
mints a fresh `ExportId` on every call, so replaying the same caller-minted
`Aex-Operation-Id` can create a second export instead of resolving the first.

Serving that handler would therefore publish two false guarantees at once. The
workspace and session export-create paths now answer the router's bare `404`
with no body and no `Location`; they perform no authentication or authority
write. The other 25 finite observation routes remain mounted, including export
read, download and revocation for a future correctly admitted export. The
authored route and dormant handler remain visible so the missing product surface
cannot be mistaken for a completed redesign.

Re-enabling admission requires one authority protocol, not another local
adapter workaround. At minimum, admission must atomically create the public
operation row in `session-authority` and the export state in
`observation-authority` with one request-intent identity; an exact replay must
return the original pair and a different intent must conflict. Every lifecycle
and cancellation transition must then update or fence both rows in one
cross-table `TransactWriteItems`, so generic operation reads and the dedicated
export read can never disagree. That redesign needs explicit table-item and IAM
review and is not hidden inside this no-schema containment.

The production-router regression drives both concrete POST paths and requires
`404`, an empty body and no `Location`. Ownership tests retain all 27 generated
finite routes while the served-set test pins the narrowed count at 25.
