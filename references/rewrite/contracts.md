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
- **241 published JSON Schema 2020-12 documents** under
  `api/generated/schemas/`, one per `SchemaId`, and **262 generated Rust types**
  in `crates/aex-wire/src/generated/models.rs`.
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
| **`valid`/`invalid`/`golden` corpus categories per `SchemaId`.** | 241 schemas x 3 categories is ~720 authored files. The loader (`aex_wire::testing::corpus`) and the floor mechanism exist and are used by the id and error corpora; arming a new category is a new floor assertion plus the cases. |
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
use aex_wire::models::*;   // 262 generated request, response and query types
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

## 7. Second pass — closing the seven contract gaps

Branch `rw/contracts-2`, off `main` after the first pass landed. Six peer streams
implemented against the generated wire and reported seven gaps; each one was
found by a stream that could not write correct code without it. All seven are
closed. Nothing is aliased, deprecated or shimmed.

The public operation count is now **146** — 27 central, 119 regional — up from
144, because R-DELETE replaces two routes with four.

### 7.1 `Materialize` and `Persist` carry a Brain-issued presigned plan

**Was:** `OperationRequest::Materialize { root: ContentHash }` and
`::Persist { include, exclude }`. The Hands guest is credential-free by
H-BOUNDARY, so it cannot resolve a hash to bytes and neither arm was
implementable. The Hands stream shipped both as *not implemented*.

**Now:** both carry a `PresignedPlan`, and `root` survives only as the identity
the call hash is over.

```rust
// crates/aex-hands-protocol/src/operation.rs
pub struct ContentEndpoint(String);                 // https://host[:port], normalized
pub enum ContentEndpointError { NotAnHttpsOrigin, NotBareOrigin, Host }
pub enum TransferDirection { Fetch, Store }
pub enum PersistPhase { Survey, Upload }
impl PersistPhase { pub const fn grant_direction(self) -> TransferDirection; }

pub struct PresignedPlan {
    url: HttpsUrl,                                  // private
    pub direction: TransferDirection,
    pub expires_at: Timestamp,
    pub max_bytes: u64,
    pub expected_digest: Option<ContentHash>,
}
impl PresignedPlan {
    pub const MAX_PLAN_BYTES: u64 = 16 * 1024 * 1024;   // content.bundle_expand
    pub fn fetch(url, expires_at, digest, max_bytes) -> Self;
    pub fn store(url, expires_at, max_bytes) -> Self;
    pub fn origin(&self) -> &str;
    pub fn authorize(&self, endpoint: &ContentEndpoint, now: Timestamp,
                     performing: TransferDirection) -> Result<&HttpsUrl, PlanRejection>;
}
pub enum PlanRejection { ForeignOrigin { expected, found }, Expired { expires_at, now },
    WrongDirection { expected, found }, MissingDigest, UnexpectedDigest,
    Unbounded { max_bytes, limit } }

OperationRequest::Materialize { root: ContentHash, plan: PresignedPlan }
OperationRequest::Persist { include, exclude, phase: PersistPhase, plan: PresignedPlan }
```

Two properties carry the security argument, and each has a test.

The guest needs no AWS credential because the grant *is* the credential, bounded
to one object, one direction, one five-minute window and one byte ceiling
(OD-17). It cannot be pointed at an arbitrary host because `url` is private and
`authorize` is the only way out of the type: the accessor demands the
`ContentEndpoint` the guest was launched with. That endpoint deliberately does
**not** travel on the request — a plan that named its own trust anchor would
authorize whichever host it chose, which is the entire failure mode. A userinfo
authority such as `https://trusted@evil.test/` fails the same comparison, so the
usual URL-parsing trick does not get past it either.

`Debug` names the origin and the bounds and never the signature; `Serialize`
emits the URL, because the guest genuinely needs it. A presigned URL is bearer
material for one object, so logging a request must not thereby log the
capability.

### 7.2 `OperationRequest::Browser`

**Was:** absent. `aex_hands_agent::session::requires_browser` existed, was
evaluated before any spawn, and could never fire.

**Now:**

```rust
OperationRequest::Browser { session: Option<GuestProcessId>, command: BrowserCommand }

pub struct BrowserViewport { pub width: u32, pub height: u32 }
pub enum BrowserCommand { Open { url, viewport, timeout_ms }, Navigate { url },
    Click { selector }, Type { selector, text }, Key { key },
    Scroll { selector, delta_y }, Wait { ms }, Screenshot,
    ReadText { selector }, Evaluate { expression }, Close }
impl BrowserCommand {
    pub const MAX_SELECTOR_BYTES: usize = 1024;
    pub const MAX_TEXT_BYTES: usize = 32_768;
    pub const MAX_EXPRESSION_BYTES: usize = 32_768;
    pub const MAX_KEY_BYTES: usize = 64;
    pub const MAX_WAIT_MS: u32 = 30_000;
    pub const OPEN_TIMEOUT_MS: RangeInclusive<u32> = 1_000..=120_000;
    pub const VIEWPORT_WIDTH: RangeInclusive<u32> = 320..=3_840;
    pub const VIEWPORT_HEIGHT: RangeInclusive<u32> = 240..=2_160;
    pub const fn needs_session(&self) -> bool;
    pub fn is_bounded(&self) -> bool;
}
impl OperationRequest {
    pub const fn requires_browser(&self) -> bool;
    pub const fn browser_target_is_coherent(&self) -> bool;
}
```

The command set is the union of plan 10 section 5.4's verbs and plan 09 section
3.6's action kinds, with plan 09's bounds. `Evaluate` is present deliberately:
the customer is root and can attach to the same debugging port regardless, so
forbidding it would be theatre. Only `Open` mints a session and every other
command names one, which `browser_target_is_coherent` decides rather than leaving
to a convention.

`requires_browser` moved onto the contract type. The exhaustive match that forces
a new arm to declare its side of the gate now lives in one place instead of two
that can disagree.

### 7.3 Windowed background-process output

**Was:** `ProcessStatus { process }`. Only a tail was expressible, so a caller
that stopped looking could never recover the backlog.

**Now:** `ProcessStatus { process, from_offset: u64, max_bytes: u32 }` with
`OperationRequest::MAX_OUTPUT_WINDOW_BYTES = 1_000_000` (plan 10 section 5.2).
The old spelling no longer decodes, so a stale sender is a typed failure rather
than a silent tail read. The guest-side paging already existed on
`Journal::read_output`.

### 7.4 R-DELETE: `clone` / `trash` / `restore` / `purge`

**Was:** `RouteId::SessionFork` and `RouteId::SessionDelete`.

**Now**, renamed rather than aliased — the old `operationId`s resolve to nothing
and a test asserts it, because an alias would let a generated client keep calling
a verb whose semantics no longer exist:

| Method | Path | operationId | pause-exempt |
| --- | --- | --- | --- |
| POST | `/api/sessions/{sessionId}/clones` | `session_clone` | no |
| POST | `/api/sessions/{sessionId}/trashes` | `session_trash` | yes |
| POST | `/api/sessions/{sessionId}/restores` | `session_restore` | no |
| POST | `/api/sessions/{sessionId}/purges` | `session_purge` | yes |

All four are `Aex-Operation-Id` admissions returning `202 Operation`; `clone`
also accepts `If-Match`. Trash starts the recovery window and purge is
irreversible, so both stay reachable while an account is paused (plan 04's exempt
set); restore is an ordinary mutation and is not. `clone` additionally declares
`session_not_idle`, which it can hit and `fork` never declared.

Schemas moved with the routes:

| Was | Now |
| --- | --- |
| `ForkFiles` | `CloneFiles` |
| `ForkCredentials` | `CloneCredentials` |
| `SessionForkRequest` | `SessionCloneRequest` |
| `SessionForkResult` | `SessionCloneResult` |
| `SessionDeleteRequest { cascade: boolean }` | `SessionPurgeRequest { cascade: PurgeCascade }` |
| — | `PurgeCascade { detach_descendants, purge_closure }` |
| — | `SessionTrashResult { sessionId, trashedAt, recoveryDeadline, sessionRevision }` |
| — | `SessionRestoreResult { sessionId, restoredAt, status, sessionRevision }` |
| `SessionLineage { parentSessionId, forkedAtPersistRevision, forkOperationId }` | `SessionLineage { originSessionId, clonedAtPersistRevision, cloneOperationId }` |

`PurgeCascade` replaces a boolean because the domain distinguishes detaching
descendants from purging the closure, and a boolean cannot carry that.

`OperationKind` loses `session_fork` and `session_delete` and gains
`session_clone`, `session_trash`, `session_restore`, `session_purge`;
`OperationResult` follows, with `session_purge` carrying the existing
`SessionTombstone`. `workspace_delete` is untouched — it is the central route and
is not in R-DELETE's scope.

`DeletingSession`, `SessionTombstone` and the `session_deleting` /
`session_deleted` / `deletion_in_progress` error codes are deliberately **kept**.
They name states, not verbs, and the regional-domains stream already maps its
rejections onto them.

Counts are pinned in `api/schemas/registries/routes-meta.yaml` (`regional: 119`)
and asserted independently by `tools/aex-contract-gen/tests/determinism.rs` and
`crates/aex-wire/tests/errors_and_routes.rs`, so the change is visible in a diff
rather than absorbed. `conformance/routes/bindings.jsonl` regenerated to 146
golden bindings.

### 7.5 `approval_binding_changed`

Added to `api/schemas/registries/errors.yaml` and therefore to `ErrorCode`:

| Field | Value |
| --- | --- |
| code | `approval_binding_changed` |
| status | `409` |
| class | `conflict` |
| retryable | `false` |
| precedence stage | `domain_state` |
| remedy | re-read the approval and decide against its current bound call |

409 / `conflict` / `domain_state` rather than 412 / `precondition`: the drift is
detected at decision time and the approval is auto-cancelled, so it is a state
conflict, not a failed caller-supplied precondition — and it sits beside
`approval_already_resolved`, the other way a decision can arrive too late.
Declared on `session_approval_respond`. `conformance/errors/cases.jsonl` gains
its case; the corpus floor is one case per code, so the case had to exist before
the code did.

### 7.6 `OutboxEvent` moved to `aex-internal-contracts`

**Was:** `aex_session_domain::terminal::OutboxEvent`, with no `Serialize` at all
— which made "both `regional-stream` and the observation materializer decode it"
a claim nothing could satisfy.

**Now:** `aex_internal_contracts::outbox`, carrying a `schema_version` and
rejecting unknown members like every other internal envelope.

```rust
pub struct OutboxEvent { pub schema_version: SchemaVersion, pub session: SessionId,
    pub run: RunId, pub status: RunStatus, pub session_revision: SessionRevision,
    pub usage_closure: UsageClosureId, pub at: Timestamp }
pub enum RunStatus { Queued, Running, Succeeded, Failed, TimedOut, Cancelled, Interrupted }
pub struct SessionRevision(pub u64);      // canonical decimal string on the wire
pub struct UsageClosureId(pub Uuid7);
```

`RunStatus`, `SessionRevision` and `UsageClosureId` moved with it: an envelope
whose members live in a crate the readers do not depend on is not decodable,
which is the whole point of moving it. `aex-session-domain` re-exports all four
from their original paths, so no call site changed.

`aex_internal_contracts::outbox::RunStatus` and the public
`aex_wire::models::RunStatus` remain two types — one internal envelope, one
customer rendering — exactly as before the move. What must never drift is their
spelling, so `tests/boundaries.rs` asserts the two agree value for value.

### 7.7 `ObservationCoverage` watermarks are `DecimalU128`

O-04 requires all four watermarks to be accepted-time positions in epoch
milliseconds. `snapshot` already was; `accepted` and `indexed` were composite
`ObservationWatermark` objects and `earliestReplay` was an RFC 3339 instant, so
the observation stream had to reconstruct a scalar it was never given. All four
are now `decimal`. `ObservationWatermark` has no remaining referent and is
**deleted**, not left orphaned.

That deletion exposed a generator hole worth recording: `check` compared only the
files the generator still produces, so
`api/generated/schemas/ObservationWatermark.json` would have kept serving
forever. `GeneratedTree` gained `stale_files`; a directory holding a generated
file is owned by the generator, so `build` now removes and `check` now reports
anything in it the generator no longer produces. `README.md` is the one permitted
authored companion.

### 7.8 Peer call sites touched

Three peer crates, all mechanical. Re-run these suites:

| Crate | What changed | Why |
| --- | --- | --- |
| `aex-session-domain` | `src/ids.rs`, `src/run.rs`, `src/terminal.rs`: the local `SessionRevision`, `UsageClosureId`, `RunStatus` and `OutboxEvent` declarations become `pub use` of the contract crate; `claim_terminal` sets `schema_version` | 7.6 |
| `aex-hands-agent` | `src/session.rs`: `requires_browser` delegates to `OperationRequest::requires_browser` instead of keeping a second exhaustive match | 7.2 |
| `aex-session-app` | `src/error.rs`: a `TODO(cross-stream)` note only, no behaviour change | 7.5 |

### 7.9 What a peer still owes

| `TODO(cross-stream)` | Owner |
| --- | --- |
| `AppError::code` must map `ApprovalRejection::BindingChanged` onto `ErrorCode::ApprovalBindingChanged` and surface the drifted field list, instead of falling through to `precondition_failed`. The code exists; nothing produces it yet. | regional domains |
| The guest browser executor is still absent. The gate now has an arm to reject, so every `Browser` operation fails closed with `capability_unavailable` until the executor lands. | hands |
| `aex-hands-tools` can now implement `Materialize` and `Persist` against `PresignedPlan::authorize`, threading the launch-time `ContentEndpoint` through the guest supervisor. Nothing implements them yet. | hands |
| The regional session API must bind the four new routes; `aex-session-app` already has `trash_session`, `restore_session`, `purge_session` and the clone plan, so this is wiring, not new logic. | regional services |
| `OperationFailure.reason` is still a free `String`. The Hands stream emits plan 10 section 3.7's stable codes but nothing enforces the closed set. Not in this pass's scope; recorded so it is not lost. | contracts, next pass |
| The three observation error codes (`unsupported_media_type`, `telemetry_query_budget_exhausted`, `export_capacity`) and a published `ExportManifest` / `ExportMember` schema were requested by the observations stream and are **not** in this pass — only the two gaps assigned here were. | contracts, next pass |

### 7.10 Decisions taken in this pass

| # | Decision | Rationale |
| --- | --- | --- |
| C-46 | The presigned URL is a private field and `authorize(endpoint, now, direction)` is the only accessor | A guest that can read the URL without presenting its pinned origin and the current instant can be pointed anywhere by a forged frame and can use a dead grant. Making the checks unavoidable is stronger than documenting them. |
| C-47 | `ContentEndpoint` is pinned at guest launch and never travels on a request | A plan that carried its own trust anchor would authorize whichever host it named, which is not a check at all. |
| C-48 | Coherence rules live in `authorize` rather than in `Deserialize` | The decoder stays a pure shape check on the hostile boundary, and an incoherent plan is simply unusable — fail-closed without a second validation pass. |
| C-49 | `PersistPhase` decides its own grant direction | Phase one stores a survey manifest and phase two fetches the blob list; letting the phase name the direction makes presenting the wrong grant a typed rejection rather than a runtime surprise. |
| C-50 | `SessionTrashResult` and `SessionRestoreResult` are new result shapes rather than a shared receipt | `OperationResult` is a tagged union over `OperationKind`; two kinds sharing one payload would make the tag non-informative, and trash genuinely carries a recovery deadline that restore does not. |
| C-51 | `DeletingSession`, `SessionTombstone` and the `session_deleting` / `session_deleted` / `deletion_in_progress` codes survive R-DELETE unrenamed | They name states, not verbs. R-DELETE renames the verbs; the regional-domains stream already maps its rejections onto these codes, and renaming them would be churn with no reader-visible gain. |
| C-52 | The generator owns whole directories, not just the files it wrote last | Comparing file by file cannot see an output whose input was deleted. `ObservationWatermark.json` was that case, and it would have kept serving from `api/generated/schemas/` indefinitely. |
| C-53 | `aex_internal_contracts::outbox::RunStatus` stays distinct from `aex_wire::models::RunStatus`, with a test pinning their spellings equal | Unifying them would put a customer-rendering type inside an internal envelope; leaving them unlinked would let a rename on one side silently break the materializer. The test is the cheap half of both. |
