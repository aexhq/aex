---
title: Contracts stream handoff — authored contract tree, generator and the four contract crates
description: What the contracts stream implemented on rw/contracts, what it deliberately left undone, every cross-stream type it publishes with its exact path, every change it needs from a peer, and every decision it took beyond the orchestrator conventions.
keywords:
  - contracts
  - openapi
  - json schema
  - code generation
  - aex-wire
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-01
related:
  - references/rust-native-rewrite-2026-07-31/plans/00-orchestrator-conventions.md
  - references/rust-native-rewrite-2026-07-31/plans/01-contracts.md
  - references/session-execution-persistence-telemetry-wire-contract-2026-07-30.md
---

# Contracts stream handoff

Branch `rw/contracts`. Everything below is on that branch and nothing is pushed.

## 1. What is implemented

### The authored contract tree, `api/`

One authored source produces every downstream artifact. There is no second place
to change a route or a shape.

```text
api/schemas/registries/   ids errors scopes limits evolution routes-meta   (.yaml)
api/schemas/{common,central,regional,observation,usage,provider}/          (.yaml)
api/openapi/plane.{central,regional}.yaml
api/openapi/{central,regional}/<fragment>.yaml
api/generated/            openapi/ schemas/ registries/ bundle.json bundle.lock.json
```

- **144 public operations**: 27 central, 117 regional. Both counts are pinned in
  `routes-meta.yaml` and the generator refuses to emit if either drifts.
- **22 identifier kinds**, **63 error codes**, **28 scopes**, **16 limits**.
- **187 published JSON Schema 2020-12 documents** under
  `api/generated/schemas/`, one per `SchemaId`.
- Two self-contained OpenAPI 3.1 plane documents.
- `bundle.json` plus `bundle.lock.json`, which records the contract digest, the
  generator identity, the pinned toolchain, and the SHA-256 of every input.

### The generator, `tools/aex-contract-gen`

`build`, `check`, `digest` and `classify`. `build` and `check` share one code
path and differ only in whether the finished in-memory tree is written or
compared.

Determinism is structural, not incidental: `BTreeMap`/`BTreeSet`/`Vec` only, no
clock, no RNG, no environment read, a sorted symlink-free walk for discovery, and
workspace-relative `/` paths everywhere so a Windows run and a Linux run produce
identical bytes. `tests/determinism.rs` asserts all of it, including that the
committed output equals a fresh generation.

The Rust renderer is a **rustfmt fixed point** by construction, following the
`aex-telemetry-schema` pattern: it mirrors `max_width`, `array_width`,
`chain_width` and `struct_lit_width`, so `cargo fmt --all` over committed
generated source is a no-op. No `rustfmt` subprocess is invoked, which also keeps
the generator free of ambient input.

The diff classifier implements every row of the plan's evolution table and
reports without ever auto-failing.

### `crates/aex-wire`

Base crate of the contract layer. Hand-written primitives plus a generated
surface under `src/generated/`.

### `crates/aex-internal-contracts`

`money`, `assertion` (including `resolve_session_for_workspace`), `control`,
`journal`, `usage`, `wake`, `observation`, `release`, `catalog`.

### `crates/aex-payment-contracts`

`command`, `result`, `event`. The five-state effect machine, the derived provider
idempotency key, and the redaction guarantees.

### `crates/aex-hands-protocol`

`rpc` (the five verbs), `operation`, `lifecycle`. Clean replacement of the
retired frame protocol, with the hostile-input decoder ordering asserted.

### Conformance corpus

```text
conformance/ids/valid.jsonl        AUTHORED   22 cases, one per registry kind
conformance/ids/invalid.jsonl      AUTHORED   132 cases, six rejection modes per kind
conformance/errors/cases.jsonl     AUTHORED   63 cases, exactly one per error code
conformance/routes/bindings.jsonl  GENERATED  144 golden path bindings
```

Floors are asserted as failing tests, not warnings: a missing id kind, a missing
rejection mode, a missing error code or a missing route binding fails the suite.

## 2. What I deliberately left undone

Each of these is a tracked gap, not an oversight. None of them is load-bearing
for a peer to start.

| Gap | Why, and what it costs |
| --- | --- |
| **The 18 generated server traits and the 144 generated client methods.** `aex_wire::server` publishes the shapes (`RequestContext`, `Created<T>`, `Accepted`, `WithETag<T>`, `NoContent`, `NdjsonStream<F>`, `AcceptKind`, `SessionReadResult`, `ErrorResponse`) but not one trait per fragment. | This was the single largest emitter and the least load-bearing: `ROUTES` plus the generated models already give the HTTP crates everything they need to bind handlers, and a trait per fragment is mechanical once the table exists. Emitting them is a contained addition to `emit_routes.rs`. |
| **`packages/sdk/src/generated/**` TypeScript emission** and the `parity` corpus category. | Depends on the SDK stream's tree existing. The Rust side of the parity property (canonical bytes, `intentDigest`) is implemented and tested; only the `TypeScript` half is absent. |
| **`valid`/`invalid`/`golden` corpus categories per `SchemaId`.** | 187 schemas × 3 categories is ~560 authored files. The loader (`aex_wire::testing::corpus`) and the floor mechanism exist and are used by the id and error corpora; arming a new category is a new floor assertion plus the cases. |
| **`routes/<operationId>/<case>/{request,response,meta}.json`.** | Replaced for now by the generated `conformance/routes/bindings.jsonl` golden, which covers all 144 operations for path binding and matcher round-trip but not request/response bodies or `intentDigest` per route. |
| **Fuzz targets** (`fuzz_targets/decode_public.rs`, `decode_agent_message`). | The hostile-input matrix is covered by explicit cases in `crates/aex-hands-protocol/tests/hostile_input.rs`; a `cargo-fuzz` target needs a nightly toolchain the workspace does not pin. |
| **Deleting `aex/packages/contracts/`, `aex/scripts/openapi/`.** | Left in place: the TypeScript surfaces are still consumed by the retained `packages/sdk` and `apps/`, and deleting them belongs with the SDK stream's cut rather than ahead of it. |
| **`ObservationFilter` bound enforcement inside `Deserialize`.** | The bounds are declared in the schema (depth via `max` on each `filters` array, ≤ 100 `in` values, ≤ 1024-byte operands) and published in the JSON Schema, but the generated decoder enforces array `max` only through the schema, not through a hand-written `Deserialize`. `aex-observation-query` must still re-validate structure until this lands. |

## 3. Every cross-stream type I publish

### `aex-wire` — every Rust consumer

```rust
use aex_wire::ids::{
    AgentId, ApiKeyId, ApprovalId, ExportId, GenerationId, InvitationId, MeasurementId,
    MembershipId, MessageId, ObservationId, OperationId, OrganizationId, ProviderCredentialId,
    RunId, SessionId, StatementId, TelemetryBatchId, TelemetryGapId, ToolCallId, UploadId,
    UserId, WorkspaceId,
    IdKind, PrefixedId, Uuid7, IdText, IdParseError, SUFFIX_LEN,
    ResourceName, FilePath, ContentHash, TraceId, SpanId, WorkspaceApiKey, ApiKeyParseError,
};
use aex_wire::error::{
    ErrorCode, ErrorClass, PrecedenceStage, ObservedErrorCode, WireError, WireResult,
    ApiError, ApiErrorBody, ErrorDetails, ErrorDetailsRequiredScope, ErrorDetailsLimit,
    ErrorDetailsQuota, ErrorDetailsRegion, ErrorDetailsRetry, ErrorDetailsOperation,
    ErrorDetailsValidation, ErrorDetailsGap,
};
use aex_wire::page::Page;
use aex_wire::cursor::Cursor;
use aex_wire::idempotency::{
    IdempotencyKey, IdempotencyKind, PrincipalKind, PrincipalScope, IntentDigest,
    OperationIdentity, ReplayIdentity,
};
use aex_wire::canonical::{to_jcs_bytes, to_jcs_string, intent_digest, CanonicalJson, CanonicalError};
use aex_wire::scopes::{ScopeId, ScopeSet};
use aex_wire::limits::{
    LimitId, LimitShape, LimitValue, LimitScalarValue, LimitMapValue, LimitSource,
    EffectiveWorkspaceLimit, EffectiveWorkspaceLimitPage,
};
use aex_wire::provider::{ProviderId, ModelSelection};
use aex_wire::routes::{
    RouteId, RouteDescriptor, ROUTES, route, match_route, PathBinding,
    Plane, BodyClass, TransportKind, EtagPolicy,
};
use aex_wire::types::{
    Region, ComputeSize, HttpMethod, Timestamp, DecimalU128, Cents, ETag, RequestId,
    StableCode, HttpsUrl, JsonPointer, ByteRange, MetadataValue, ValueError,
};
use aex_wire::server::{
    RequestContext, AcceptKind, Created, Accepted, WithETag, NoContent, NdjsonStream,
    SessionReadResult, ErrorResponse, NextPage,
};
use aex_wire::testing::corpus;
use aex_wire::models::*;   // 187 generated request, response and query types
```

`aex_wire::models` includes, among others: `Session`, `SessionListItem`,
`SessionCreateRequest`, `ResolvedConfig`, `ResolvedCompute`, `WorkspaceContinuity`,
`SessionLineage`, `DeletingSession`, `SessionTombstone`, `Message`, `MessagePart`,
`MessageSendRequest`, `MessageSendResult`, `Run`, `Operation`, `OperationResult`,
`OperationKind`, `OperationStatus`, `DownloadGrant`, `FileEntry`,
`RegisteredFile`/`Skill`/`Tool`/`Instruction`/`McpServer` plus their `*Value` and
`*Page` forms, `BlobInput`, `Upload`, `SecretMetadata`, `ProviderCredential`,
`Approval`, `ObservationQuery`, `ObservationFilter`, `ObservationPage`,
`ObservationCoverage`, `ObservationFrame`, `TelemetryGap`, `TelemetryExport`,
`MetricAggregationRequest`, `TraceDetail`, `UsageQuery`, `UsageAggregate`,
`UsageFrontier`, `UsagePage`, `Organization`, `Membership`, `Invitation`,
`Workspace`, `ApiKey`, `NewApiKey`, `BillingBalance`, `Statement`,
`AutoTopupPolicy`, `DeviceAuthorization`, `DeviceToken`, `DashboardBootstrap`.
Per-operation query structs are `<RouteIdVariant>Query`, for example
`SessionsListQuery`, `WorkspacesListQuery`, `UsageQueryQuery`.

### `aex-internal-contracts`

```rust
use aex_internal_contracts::{SchemaVersion, Epoch, FifoGroup, DedupeKey, PricingVersion};
use aex_internal_contracts::money::{Microusd, MicrousdDelta, MoneyError, MICROUSD_PER_CENT};
use aex_internal_contracts::assertion::{
    AuthorizationAssertion, AssertionAudience, AssertionError, signing_input, MAX_LIFETIME_MS,
    ResolveSessionForWorkspace, ResolvedSessionAssertion,
};
use aex_internal_contracts::control::{ControlCommand, ControlEnvelope, EmailKind};
use aex_internal_contracts::journal::{
    JournalEntryKind, JournalEnvelope, JournalSubject,
};
use aex_internal_contracts::usage::{
    Meter, FactBasis, ServiceTime, AuthorityKind, FactAuthority, FactId, Attribution,
    SourceReceipt, FactIdempotency, UsageFact, FactInboxKey, SettlementReceipt,
};
use aex_internal_contracts::wake::{WakeHint, WakeEnvelope};
use aex_internal_contracts::observation::{
    ReceiptState, AdmissionReceipt, SpoolChunkRef, ExportMember, ExportManifest,
};
use aex_internal_contracts::release::{ArtifactManifest, CompositionManifest, EvidenceReceipt};
use aex_internal_contracts::catalog::CatalogRevision;
```

### `aex-payment-contracts`

```rust
use aex_payment_contracts::{
    ProviderObjectRef, ProviderCustomerRef, ProviderMethodRef, ProviderChargeRef, RedactedEmail,
};
use aex_payment_contracts::command::{
    EffectId, CommandKind, ProviderIdempotencyKey, PaymentCommand, PaymentCommandEnvelope,
    EffectMetadata,
};
use aex_payment_contracts::result::{
    EffectState, TaxMode, TaxEvidence, PaymentFailureClass, PaymentFailure, HostedSession,
    UnknownEvidence, PaymentResult,
};
use aex_payment_contracts::event::{
    ProviderEventId, PinnedApiVersion, ProviderEventKind, ProviderEventFacts,
    ProviderEventEnvelope, ProviderInboxKey, IdempotencyIntent,
};
```

### `aex-hands-protocol`

```rust
use aex_hands_protocol::rpc::{
    HandsOperationId, CallHash, Fence, GuestRevision, OutputStream, CancelReason,
    GenerationBinding, GenerationExpectation,
    StartRequest, StartResponse, StatusRequest, StatusResponse,
    CancelRequest, CancelResponse, ResultRequest, ResultResponse, ResultChunk,
    AttachedEvent, HandsMessage, decode_agent_message, MessageDecodeError,
};
use aex_hands_protocol::operation::{
    GuestRoot, GuestPath, GuestPathError, EnvName, EnvValue, FileMode, StopSignal,
    GuestProcessId, ByteRangeRequest, SearchPattern, Patch, PatchHunk, ContentRef,
    OperationRequest, OperationBounds, TerminalState, OperationExit, OperationFailure,
    TerminalMetadata, DeliveryMode,
};
use aex_hands_protocol::lifecycle::{
    ProviderReceiptId, ProviderRequestId, ProviderFailure, KeepaliveLease,
    LifecycleIntent, LifecycleOutcome, ReceiptError, RuntimeReceipt, TrueIdleEvidence,
};
```

## 4. What I need from a peer

| `TODO(cross-stream)` | Owner |
| --- | --- |
| `aex-model-catalog` must expose `fn qualified(&self, sel: &ModelSelection) -> Result<QualifiedModel, CatalogError>`. I define neither `QualifiedModel` nor `CatalogError`; I publish `ProviderId` and `ModelSelection` and the `unknown_provider` / `unknown_model` / `unqualified_provider_model` codes it will report through. | providers |
| The observations stream owns the *production-side* assertion that `TelemetryGapReason::ReplayExpired` is never emitted. My test asserts only that nothing in the public contract lets a caller submit a gap and that the variant still decodes. A reachability assertion needs the reconciler. | observations |
| `aex-observation-query` must re-validate `ObservationFilter` structural bounds until the generated decoder enforces them itself (see §2). | observations |
| The SDK stream must exclude `packages/sdk/src/generated/**` from ESLint and Prettier and must not re-export generated symbols from the package root unless they are deliberately supported. Nothing is written there yet. | clients |
| The finance stream should confirm that settlement still happens on `payment_intent.succeeded` under the new `EffectMetadata`; `checkout.session.completed` is deliberately absent from `ProviderEventKind`. | finance |
| The delivery stream owns the actual Stripe API version pin behind `PinnedApiVersion`, and the `LIVE_TARGETS` derivation. Release schemas are at `api/schemas/release/` per §8a — the directory exists but the three release shapes are currently Rust-only in `aex_internal_contracts::release`; publishing them as JSON Schemas is a small addition once delivery confirms the field set. | delivery |
| Deployables should read `RouteDescriptor::{required_scope, alt_principal, idempotency, body_class, transport, etag, success_status, safe_retry, pause_exempt, errors}` and enforce the whole precedence order from that one table rather than re-deciding per handler. | all deployables |

I also touched **one file outside my declared ownership**: `.gitattributes`, to
add `api/generated/**`, `api/openapi/**`, `api/schemas/**` and `conformance/**`
as `text eol=lf`. Without it `core.autocrlf` rewrites the committed artifacts to
CRLF on a Windows checkout while the generator writes LF, and
`aex-contract-gen check` fails on a clean tree. The file already carried the same
rule and the same rationale for `*.rs`.

## 5. Decisions I took beyond the orchestrator conventions

| # | Decision | Rationale |
| --- | --- | --- |
| C-32 | Schemas are **authored** in a compact YAML dialect under `api/schemas/**`; the **published** `api/generated/schemas/<SchemaId>.json` is real JSON Schema 2020-12. | Plan §2.1 authored raw JSON Schema. One source now produces both the schema and the Rust type, so the two cannot disagree, and the authored form is roughly a fifth of the volume. The published artifact is unchanged in kind. |
| C-33 | The generated Rust renderer is a rustfmt fixed point rather than a `rustfmt` subprocess. | §3.3 forbids ambient input, and shelling out to a formatter is ambient input. `aex-telemetry-schema` had already established the pattern, and the drift test proves it holds. |
| C-34 | `bundle.lock.json` records the pinned toolchain channel read from `rust-toolchain.toml` and a fixed formatter identity, not a resolved `rustc --version`. | Same reason: running the compiler to describe the build reintroduces exactly the ambient read §3.3 removes. The channel is a committed file. |
| C-35 | Every generated Rust file carries three inner `allow` attributes with reasons: `clippy::large_enum_variant`, `clippy::match_same_arms`, `clippy::too_many_lines`. | A registry table has one arm and one variant per row by construction. Collapsing two arms that happen to share a value today would hide the row, which is the opposite of what an auditable table is for. |
| C-36 | `LimitValue` is a **tagged** union on `shape` rather than the accepted contract's untagged `number \| Record<string, number>`. | The plan's own mapping table rejects an untagged union outright. A discriminated value is also the only form that survives a strict decoder. Flagged as a wire change from the accepted contract. |
| C-37 | `ResolvedCompute` is one object carrying `size` plus the derived capacity, not a five-arm union. | The five arms have an identical field set and differ only in constant values, so the union bought documentation and cost five schemas plus five Rust types. The values remain canonical in the limits decision. |
| C-38 | `SessionReadResult` is a server-side union in `aex_wire::server`, not a wire schema. | The discriminator is the HTTP status (200 versus 410), not a body member, so an internally tagged wire union would have had to invent a tag the contract does not have. |
| C-39 | `aex-wire` compiles all modules unconditionally; the planned `server`, `client`, `testing`, `models` and `routes` features are removed. | None of them added a third-party dependency, so they gated visibility only — and a feature that gates nothing but visibility is a footgun for twelve peers who each have to remember it. |
| C-40 | `resolve_session_for_workspace` is an `aex-internal-contracts::assertion` envelope pair, not a public HTTP route. | `central-authz` is an internal service reachable only from the regional plane; putting its operation in the public bundle would advertise a route no customer can call. The types are published so the identity and control streams can implement it. |
| C-41 | The conformance corpus uses JSON Lines files per category (`ids/valid.jsonl`, `errors/cases.jsonl`) rather than a file per case. | 63 error codes and 154 id cases as individual files is 217 files whose diffs nobody reads. One line per case keeps the review surface honest. |
| C-42 | A route's declared error slice is sorted in **registry** order, not alphabetically. | The generated `ErrorCode` discriminant follows the registry, so a route slice sorted any other way cannot be binary-searched or compared against `ErrorCode::ALL` without a re-sort at every call site. |
| C-43 | The generator's JCS normalizes an integral float to an integer, matching `aex-wire`. | `1.0` and `1` are the same `ECMAScript` number. `tests/canonical_agreement.rs` asserts the two implementations agree on every vector, which is the property that makes carrying two implementations safe. |
| C-44 | Four operations were added beyond the plan's 137: `device_authorization_create`, `device_token_create`, `dashboard_bootstrap_get`, and the four provider-credential routes — 144 total. Two error codes were added for the device flow (`authorization_pending`, `slow_down`). | All four gaps were named as binding in §8a. The counts are pinned in `routes-meta.yaml` so the addition is visible in a diff rather than absorbed. |
| C-45 | `Timestamp` rejects a magnitude at or beyond `1e21` in JCS and rejects every RFC 3339 spelling except `YYYY-MM-DDThh:mm:ss.sssZ`. | `1e21` is exactly where `ECMAScript`'s `Number::toString` switches to exponential notation and a Rust shortest-round-trip printer does not, so it is the point past which two canonicalizers would silently disagree. |

## 6. Gate output

Recorded in the stream report. All five gates pass:
`cargo fmt --all`, `cargo clippy` over the five owned crates with `-D warnings`,
`cargo nextest run` over the five owned crates, `cargo check --workspace
--all-targets`, and `cargo run -p aex-workspace-check`.
