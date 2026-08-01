---
title: Regional services rewrite handoff
description: Implementation and merge handoff for the regional-services stream.
status: implemented-with-deferred-runtime-composition
owner: regional-services
---

# Regional services rewrite handoff

## Implemented

The branch is `rw/regional-services`. It implements the stream depth-first in
the accepted work order:

- `aex-regional-http`: the sole `aex_wire::canonical`-based idempotency and
  cursor input path; signed `cur_` cursors with the full binding and 24-hour
  expiry; fixed 10 MiB envelope and effective body limits; full edge error
  precedence; redaction; assertion verification and a bounded single-flight
  cache; sealed compile-time grant tokens; fail-closed config/capability
  admission; bounded pagination; whole-frame NDJSON writes; sent-cursor
  tracking; total generated regional-route ownership; and shared internal
  health/readiness routes.
- `regional-session-api`: a generated-route partition and the exact eight-action
  admission transaction over `AuthzProjection`, `SessionAuthority`, and
  `RegionalWork`. The `ClientRequestToken` is deterministic and bound to the
  canonical idempotency identity and command resource ids. The plan has no
  queue action.
- `regional-secret-api`: the four plaintext-admission routes, redacted and
  zeroizing plaintext/ciphertext wrappers, encryption before return, strict
  `If-Match`, and monotonic generation/revocation models.
- `session-operation-worker`: only purge and paged persist/fork continuations;
  leased monotonic fences; stable effect ids; one cursor step per commit;
  duplicate and stale-fence rejection; deterministic due shards; exact attempt
  eight manual-review terminalization; SQS partial-batch results; and durable,
  projected deletion-denial gating before a tombstone.
- `content-lifecycle-worker`: exact 24-hour staged grace; every owner/root/grant/
  operation pin recheck; deletion-denial recheck under a fence; exact
  unversioned object key, `If-Match`, and explicit expected bucket owner;
  precondition-mismatch handling; and mode-specific delete capability
  admission.
- `regional-stream`: closed wake modes and the two-task `ddb_streams` assertion;
  authoritative reads after wake hints; one origin for stream and none for
  listen; separate session/observation/telemetry quotas; dual telemetry charge;
  drain readiness and exact reconnect cursors; and ownership of all 24 current
  generated NDJSON routes with no mutation dependency.
- `regional-secret-key-admin`: exclusive generation lineage, idempotent current
  generation, conditional rotation, startup resource/region/account/operation
  binding, and rejection of product-request bindings.
- Deterministic performance-contract targets for all six deployables, plus the
  `LOAD-STREAM-SOCKETS` workload descriptor with the full eighteen-metric set.

The final owned-package test command runs 83 tests with no skip. The source
contains no ignored tests, retry-to-green path, quarantine, custom
canonicalizer, floating-point resource quantity, or cloud call.

## Deliberately deferred

These items remain and are not represented as complete:

- The six existing binary skeletons still fail fast with
  `RunError::NotImplemented`. Generated `aex_wire` server traits/router
  constructors and the peer command/store ports needed to compose real
  `lambda_http`/`hyper` entrypoints are absent on the branch base. Therefore the
  shared health router is implemented and tested but is not yet mounted by a
  deployable listener.
- The six live-companion packages remain deployment-phase skeletons. No remote
  AWS, socket load, drain/replacement, or IAM-denial evidence was fabricated;
  `release/unearned-evidence.json` continues to record it. The stream socket
  workload contract is authored, but its live `Driver` executor remains due in
  `aex-live-regional-stream` once a deployed descriptor exists.
- `content-lifecycle-worker` has the conservative staged/delete kernel, but the
  complete mark/sweep/inventory orchestration and actual DynamoDB/S3 adapters
  are not wired.
- `regional-stream` has the state machines and generated route partition, but
  not the ALB listener, live DynamoDB Streams reader, bounded network queue, or
  SIGTERM task-drain loop.
- `regional-secret-key-admin` has the key-lineage kernel and startup admission,
  but not the final clap command/exit-code-3 binary or real KMS/DynamoDB adapter.
- The eight-row unknown-outcome recovery matrix for durable regional commands
  needs the peer transaction/store result vocabulary before it can be asserted
  against durable facts.

These were deferred to preserve a tested, compiling depth-first kernel rather
than introduce fake adapters or compatibility shims for missing peer APIs.

## Public types for peers

The cross-deployable API published by this stream is under
`aex_regional_http`:

- `aex_regional_http::assertion::{PresentedCredential, SignedAssertion,
  ProjectedEpochs, VerifiedAuthorization, KeyVerifier, AssertionSource,
  AuthFailure, VerifyingAssertionCache}`
- `aex_regional_http::capability::{Capability, Declares, Grant,
  SecretPlaintextAdmission, SecretDecrypt, KeystoreAdminister,
  ContentObjectDelete, ContentEncrypt, WorkClaim, StreamSocket, DeployableId,
  CapabilityBinding, CompositionManifest, ResolvedConfig, CompositionError}`
- `aex_regional_http::context::{AuthorizationEpochs, AccountState,
  RegionalAuthorization, EffectiveLimits, RequestContext}`
- `aex_regional_http::cursor::{Order, SnapshotToken, CursorBinding, SortTuple,
  CursorKey, CursorKeyRing, CursorError}`
- `aex_regional_http::envelope::EnvelopeError`
- `aex_regional_http::error::{EdgeError, IntoWireError}`
- `aex_regional_http::health::{Readiness, ReadinessError}`
- `aex_regional_http::idempotency::{IdentityContext, IdempotencyIdentity,
  IdentityError}`
- `aex_regional_http::limits::{BodyLimits, LimitError}`
- `aex_regional_http::page::{Page, Paginator, PageError}`
- `aex_regional_http::router::{RouteOwner, EdgeStack}`
- `aex_regional_http::stream::{RotateReason, Frame, FrameSink, SentCursor,
  FrameWriter, StreamWriteError, FrameSplitError}`

The deployable crates also expose pure kernels for their future composition
roots:

- `regional_session_api::admission::{Table, Condition, TransactionAction,
  AdmissionInput, AdmissionError}`
- `regional_secret_api::{SecretPlaintext, Ciphertext, SecretRecord,
  AdmissionError}`
- `regional_stream::{WakeMode, StreamConfigError, StreamOrigin, OriginError,
  ConnectionClass, QuotaLimits, QuotaManager, Reservation, QuotaError,
  DrainFrame, DrainCoordinator}`
- `session_operation_worker::{WorkKind, WorkStatus, WorkRecord, Claim, StepPlan,
  BatchItem, BatchResponse, DeletionDenial, WorkError, ClaimError, CommitError}`
- `content_lifecycle_worker::{ContentState, ContentItem, ReconcileOutcome,
  DeletionDenial, DeleteIntent, DeleteOutcome, ObjectDeleteResult, Mode,
  ModeAdmission, ModeError, LifecycleError}`
- `regional_secret_key_admin::{AdminOutcome, GenerationRecord, KeyAdmin,
  StartupBinding, KeyAdminError}`

## `wire_pending` inventory

Every local placeholder is marked at its definition with the required merge
comment:

- `aex_regional_http::wire_pending::RegionalSessionApi` replaces
  `aex_wire::server::RegionalSessionApi`.
- `aex_regional_http::wire_pending::RegionalSecretApi` replaces
  `aex_wire::server::RegionalSecretApi`.
- `aex_regional_http::wire_pending::RegionalStreamApi` replaces
  `aex_wire::server::RegionalStreamApi`.
- `regional_session_api::wire_pending::TransactionPlan<A>` replaces
  `aex_session_app::TransactionPlan`.

## Peer changes required

- Contracts must generate the three server traits/router constructors listed
  above. The current generated wire exposes route descriptors and models, not a
  server composition API.
- `aex_session_app` must publish the command service and transaction-plan port
  used by admission. Its final plan needs to preserve the exact eight actions
  and accept the already-derived client request token.
- `aex_internal_contracts::assertion::AuthorizationAssertion` carries principal,
  organization, workspace, scopes, region and the three epochs, but does not
  carry the credential digest/binding or account state required by the regional
  edge plan. This branch binds the credential cryptographically in
  `SignedAssertion`; the contracts owner should add the final field vocabulary
  before the placeholder envelope is removed.
- The wire error vocabulary needs explicit `stream_capacity_exceeded` and
  `operation_manual_review` codes. The current exhaustive mapping can only use
  the nearest existing typed codes until those variants land.
- Regional stores must supply the conditional transaction, due-scan/claim,
  content-fence and secret-custody adapters; observations must supply the
  authoritative query/listen reader; infrastructure must supply the
  DynamoDB-Stream-to-Pipe hint, ALB split and internal-listener policy.

## Decisions

- Route ownership is derived exhaustively from the generated route table. The
  current wire contains exactly 24 regional NDJSON routes; observation and OTLP
  routes are assigned to their peer deployables, not silently mounted here.
- Plaintext provider-credential registration is owned by the secret API;
  metadata list/get/revoke stays on the session API except for the generated
  revocation operation that carries secret-edge write authority.
- A wake is only a hint. The payload is never emitted and a cursor advances only
  after a whole encoded frame is written and flushed.
- `ExpectedBucketOwner` is an explicit required input. No account or resource
  identifier is inferred from an environment default.
- Local performance contracts use a non-reserved `performance_contract` target;
  the reserved `load` target remains exclusively for live companions under
  D-11.

## Commits

- `b42d4ad9 feat(regional): implement fail-closed edge primitives`
- `c9cd091c feat(regional): compose session and secret admission`
- `57d9ead1 feat(regional): add fenced lifecycle workers`
- `a629232d feat(regional): enforce stream and key admin safety`
- `c40f17ab feat(regional): add total route ownership and readiness`
- `3ab90770 test(regional): add bounded performance contracts`
- `a2aabb8c test(regional): declare stream socket budget`

## Gate output

`CARGO_BUILD_JOBS=4` was set for every Cargo gate.

`cargo fmt --all` produced no output and exited `0`.

The seven individual `cargo clippy -p <owned-package> --all-targets -- -D warnings`
commands exited `0`; their final combined terminal output was:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.08s
    Checking regional-session-api v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\services\regional-session-api)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.54s
    Blocking waiting for file lock on package cache
    Checking regional-secret-api v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\services\regional-secret-api)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.94s
    Checking regional-stream v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\services\regional-stream)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.99s
    Checking session-operation-worker v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\workers\session-operation-worker)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.21s
    Checking content-lifecycle-worker v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\workers\content-lifecycle-worker)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.01s
    Checking regional-secret-key-admin v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\workers\regional-secret-key-admin)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.01s
```

The owned-package `cargo nextest` output ended exactly:

```text
────────────
     Summary [  12.124s] 83 tests run: 83 passed, 0 skipped
```

`cargo check --workspace --all-targets` ended exactly:

```text
    Checking content-lifecycle-worker v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\workers\content-lifecycle-worker)
    Checking regional-secret-key-admin v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\workers\regional-secret-key-admin)
    Checking regional-secret-api v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\services\regional-secret-api)
    Checking regional-session-api v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\services\regional-session-api)
    Checking regional-stream v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\services\regional-stream)
    Checking session-operation-worker v0.1.0 (C:\Users\luowe\AppData\Local\aex\worktrees\aex\regional-services\workers\session-operation-worker)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.92s
```

`cargo run -p aex-workspace-check` ended exactly:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.68s
     Running `target\debug\aex-workspace-check.exe`
aex-workspace-check: 133 member(s) and 139 package(s) satisfy every structural and registry rule
aex-workspace-check: 563 unearned-evidence row(s) recorded in the source-rewrite phase
```

The final registry regeneration ended exactly:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.30s
     Running `target\debug\aex-workspace-check.exe registry build`
aex-workspace-check: wrote release/test-registry.json and release/unearned-evidence.json
```
