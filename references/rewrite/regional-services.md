---
title: Regional services rewrite handoff
description: Implementation and merge handoff for the regional-services stream.
status: accepted
owner: regional-services
keywords:
  - regional http
  - stream
  - lifecycle workers
  - idempotency
  - composition
audience: implementation agents and maintainers
last_verified: 2026-08-02
related:
  - references/rewrite/regional-domains.md
  - references/rewrite/regional-stores.md
  - references/rewrite/contracts.md
---

# Regional services rewrite handoff

Plan of record: `references/rust-native-rewrite-2026-07-31/plans/06-regional-services.md` in the parent workspace.

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

## Composition

Branch `rw/deploy-regional`, off `main` after the four-stream merge. This closes
the largest tracked gap in "Deliberately deferred": the six binaries no longer
return `RunError::NotImplemented`. Each one now validates its own configuration,
builds its real adapters from it, and reaches its real entry point.

### The mount table is a projection of the route table

`aex_regional_http::router::RouteOwner` replaces the free `route_owner` function
as the ownership surface. `RouteOwner::routes()` is the owned partition of the
generated regional table, `RouteOwner::routes_in(group)` is the subset of one
authoring fragment, and `mount_unary` iterates the *served* projection rather
than listing templates. Two properties follow and are asserted:

- `route_ownership_partitions_the_regional_route_table` — the five owners'
  slices are disjoint and their union is every regional route, so a new route
  lands on a deployable by construction.
- `the_owned_set_is_drawn_from_the_groups_and_not_from_a_second_list` — walking
  `RouteGroup::ALL` and filtering by owner reproduces the owned set exactly.

`regional:secrets` and `regional:provider-credentials` are the two fragments that
span two deployables, which is why mounting filters a group rather than mounting
a whole trait: the plaintext half is the secret edge's and the metadata half is
the session API's. `not_served` is the typed refusal for the other half, and it
is unreachable through the router because only owned templates are mounted.

| Deployable | Owns | Mounts today |
| --- | --- | --- |
| `regional-session-api` | 61 | 0 |
| `regional-secret-api` | 4 | 0 |
| `regional-stream` | 24 | 0 |
| `regional-observation-api` (peer) | 27 | — |
| `regional-otlp` (peer) | 3 | — |

`UnaryDispatch::served()` defaults to the whole owned set and is narrowed only
while a named peer capability is owed. It is narrowed to the empty set on both
APIs today, and the reason is exactly one gap — see "The one blocking gap".
RS-18 is the reason this is a *narrowed mount* rather than a mounted route that
answers a permanent failure.

### The shared edge

`aex_regional_http::edge::RegionalEdge` is one implementation of the precedence
stages every finite regional deployable runs: credential presentation, the
regional projection read, credential-bound assertion verification, immutable
placement, the route's declared scope, the pause gate, the effective body bound
and strict replay identity in both directions. Every decision comes from the
generated descriptor — `required_scope`, `pause_exempt`, `idempotency` — so a
route cannot skip a stage by omission. `AssertionSource`, `KeyVerifier`,
`ProjectionReader` and `EdgeClock` stay ports; the composition root supplies the
concrete adapters.

### Configuration

`aex_regional_http::config` holds the primitives: `required`, `optional`,
`forbidden`, `positive_u64`, `bounded_u64`, `bounded_usize`, `plane_name`,
`region`, `arn_in_region`, `queue_url`, `one_of`, plus `Arn` and the `Lookup`
seam that lets a test supply a map without `unsafe` environment mutation.
Nothing has a default, every refusal names its variable, and two classes of
mistake that otherwise survive deployment are refused at start-up:

- **a resource in another region.** `arn_in_region` and `queue_url` compare the
  ARN's or the queue URL's region against the plane binding.
- **a binding the deployable must never hold.** Each deployable declares a
  `FORBIDDEN` list, and the process refuses to start when one is present. This
  is the configuration half of RS-09; IAM is the other half, and neither is a
  single point of failure.

### What each deployable now does

- **`regional-session-api`** — validates 21 variables, refuses a queue URL or the
  secret KMS key, requires the content bucket owner to equal the content key's
  account, builds the work, content, registry, custody and runtime-activity
  adapters plus the object binding, derives readiness from the composition and
  serves `/internal/healthz` and `/internal/readyz` under `lambda_http`.
- **`regional-secret-api`** — validates 13 variables, refuses any session,
  content, bucket, work, registry or queue binding, builds the custody adapter
  and the envelope crypto over the *secret* key with a plane- and region-scoped
  cache partition, and serves the health endpoints under `lambda_http`.
- **`regional-stream`** — validates 21 variables, refuses every mutating binding,
  admits `ddb_streams` only at `AEX_STREAM_MAX_TASKS <= 2` (RS-05), requires both
  authority stream ARNs in that mode, refuses a total connection budget below a
  class budget, binds the listener only after admission, and drains on `SIGTERM`
  by flipping readiness to `503` before the deadline runs.
- **`session-operation-worker`** — validates 16 variables including both queue
  regions, builds the work, content and registry adapters, and serves both
  triggers: an SQS batch answered with a partial-batch response (RS-20) and the
  scheduled sharded due scan. `Trigger::classify` is structural, and an
  unrecognised payload fails rather than draining the queue silently.
- **`content-lifecycle-worker`** — one binary, four roles. The mode selects the
  required variable set, the S3 client is constructed only in `delete` mode, and
  the object-delete capability is refused in both directions: `delete` cannot
  start without it and no other role may hold it.
- **`regional-secret-key-admin`** — the three `clap` commands, the exit-code
  interface (`0` acted, `3` already current, `1` refused), and start-up denial of
  a table that is not the configured keystore, a cross-region key, an unattested
  run and every product-table binding.

### The one blocking gap

No handler is mounted on either API because **no crate on `main` projects a
domain value onto a wire model**. `aex-session-app` returns
`Planned<(Message, Run)>` over `aex_session_domain` types; `aex-session-dynamodb`
commits `wire_pending::AdmissionPlan`; the generated `SessionsApi::session_get`
returns `aex_wire::models::Session`. Nothing converts between them, and the
adapters do not carry every field the wire models require — `ProviderCredential`
is the clearest case: the custody codec stores `credential`, `provider`,
`secret_name`, `source_generation`, `state` and `created_at`, while the wire
model additionally requires `fingerprint`, `name`, `revision` and `updated_at`.

A grep for `aex_wire::models::` outside `aex-wire` returns only enum re-uses
(`TelemetryGapReason`, `ObservationSignal`, `ObservationOrder`,
`CredentialRebindResult`, `SecretRef`). The projection layer is unowned, not
unfinished.

Two smaller consequences, both recorded rather than worked around:

- `aex-secret-custody-dynamodb::expressions` publishes `set`, `revoke`,
  `admit_custody` and `authorize_managed_call` but no conditional *delete*, so
  `secret_delete` has no adapter expression even though `aex_secret_domain`
  publishes the pure `delete`.
- `GET /api/downloads/{measurementId}` (RS-04) is not in the generated route
  table, so small-body redemption has no route to mount.

### Cross-stream requirements this pass raises

| Requirement | Owner stream |
| --- | --- |
| A domain-to-wire projection for the regional models. Whether it lands in `aex-session-app`, in the adapters, or in a new crate is an architecture decision this stream does not own, but until it exists no regional route can be mounted. | regional domains + regional stores |
| `aex-secret-custody-dynamodb::expressions::delete`, the conditional custody delete that `secret_delete` commits. | regional stores |
| The `central-authz` invoke request/response shapes for a **workspace key**. `aex-internal-contracts::assertion` publishes `ResolveSessionForWorkspace`/`ResolvedSessionAssertion` for a browser session only, so a concrete `AssertionSource` cannot be written without inventing the workspace-key payload. | central identity |
| `aws-sdk-lambda` in `[workspace.dependencies]`, which the concrete `AssertionSource` needs and no member currently declares. | delivery |
| `graph verify` reports one pre-existing violation unrelated to this stream: `[graph-cycle] cargo:aex-usage-application -> cargo:aex-usage-application`. | usage metering |

### Decisions taken beyond section 10

| # | Decision | Rationale |
| --- | --- | --- |
| RS-21 | Route ownership becomes `RouteOwner`, a projection of `ROUTES` with `routes()`, `routes_in(group)` and `groups()`, rather than a free function returning an owner | The mount loop, the composition tests and the peer assignment all need the *set*, not one lookup. Deriving the set makes an authored-but-unmounted route a red suite instead of a runtime `404`. |
| RS-22 | `UnaryDispatch::served()` may narrow the owned set, and `mount_unary` validates the narrowing | RS-18 forbids mounting a route that cannot be fully served. Without a narrowing seam the only alternatives were a permanently failing mounted route or an unbuildable binary. |
| RS-23 | Every deployable declares a `FORBIDDEN` variable list and refuses to start when one is bound | The capability boundary was previously an IAM fact only. A start-up refusal is cheaper to test, is visible without an AWS account, and fails the same way in a local run as in production. |
| RS-24 | An ARN or queue URL whose region differs from `AEX_REGION` refuses the process | A cross-region resource passes every type check and deploys cleanly; the only symptom is a tenant's data outside its declared residency. |
| RS-25 | `content-lifecycle-worker` declares `AEX_DECLARED_CAPABILITIES=none` rather than an empty string when it holds nothing | "Declared nothing" and "forgot to declare" must not be the same value; only one of them is a deliberate statement. |
| RS-26 | Readiness is derived from the composition (`Stores::unresolved`, `Readers::unresolved`) rather than declared as a constant | A readiness endpoint that reports `ready` from a literal cannot fail closed over a half-built process. |
| RS-27 | The `regional-secret-key-admin` unit is a Fargate task shape with `port = 0` | It is a one-shot task, not a service; `graph verify` only requires a non-zero port for `rust-oci-service`. |

### Environment variables per deployable

Every name below is required unless marked. There is no default for any of them.

**Common to all six**: `AEX_PLANE` (`dev`/`prd`), `AEX_REGION`,
`AEX_RELEASE_DIGEST` (the key admin excepted — it has no readiness endpoint).

| Deployable | Additional variables |
| --- | --- |
| `regional-session-api` | `AEX_AUTHZ_FUNCTION_ARN`, `AEX_AUTHZ_VERIFY_KEYS_PARAM`, `AEX_AUTHZ_PROJECTION_TABLE`, `AEX_SESSION_TABLE`, `AEX_WORK_TABLE`, `AEX_CONTENT_TABLE`, `AEX_REGISTRY_TABLE`, `AEX_SECRET_CUSTODY_TABLE`, `AEX_RUNTIME_ACTIVITY_TABLE`, `AEX_USAGE_QUERY_TABLE`, `AEX_CONTENT_BUCKET`, `AEX_CONTENT_BUCKET_OWNER`, `AEX_CONTENT_KMS_KEY_ARN`, `AEX_CURSOR_SIGNING_KEY_REF`, `AEX_ASSERTION_CACHE_BYTES`, `AEX_MAX_JSON_BODY_BYTES`, `AEX_MAX_PAGE_ITEMS`, `AEX_MAX_PAGE_BYTES` |
| `regional-secret-api` | `AEX_AUTHZ_FUNCTION_ARN`, `AEX_AUTHZ_VERIFY_KEYS_PARAM`, `AEX_AUTHZ_PROJECTION_TABLE`, `AEX_SECRET_CUSTODY_TABLE`, `AEX_SECRET_KEYSTORE_TABLE`, `AEX_SECRET_KMS_KEY_ARN`, `AEX_SECRET_BRANCH_KEY_CACHE_BYTES`, `AEX_SECRET_BRANCH_KEY_CACHE_TTL_MS`, `AEX_ASSERTION_CACHE_BYTES`, `AEX_MAX_JSON_BODY_BYTES` |
| `regional-stream` | `AEX_STREAM_PORT`, `AEX_AUTHZ_FUNCTION_ARN`, `AEX_AUTHZ_VERIFY_KEYS_PARAM`, `AEX_AUTHZ_PROJECTION_TABLE`, `AEX_SESSION_TABLE`, `AEX_OBSERVATION_TABLE`, `AEX_CONTENT_BUCKET`, `AEX_CURSOR_SIGNING_KEY_REF`, `AEX_STREAM_WAKE_MODE`, `AEX_STREAM_MAX_TASKS`, `AEX_STREAM_MAX_CONNECTIONS`, `AEX_STREAM_MAX_CONNECTIONS_SESSION`, `AEX_STREAM_MAX_CONNECTIONS_OBSERVATION`, `AEX_STREAM_MAX_CONNECTIONS_PER_WORKSPACE`, `AEX_STREAM_CONNECTION_BUFFER_BYTES`, `AEX_STREAM_WRITE_STALL_MS`, `AEX_STREAM_DRAIN_DEADLINE_MS`, `AEX_ASSERTION_CACHE_BYTES`; plus `AEX_SESSION_TABLE_STREAM_ARN` and `AEX_OBSERVATION_TABLE_STREAM_ARN` in `ddb_streams` mode only |
| `session-operation-worker` | `AEX_OPERATION_QUEUE_URL`, `AEX_OPERATION_DLQ_URL`, `AEX_WORK_TABLE`, `AEX_SESSION_TABLE`, `AEX_CONTENT_TABLE`, `AEX_REGISTRY_TABLE`, `AEX_CONTENT_BUCKET`, `AEX_CONTENT_BUCKET_OWNER`, `AEX_DENIAL_PROJECTION_TABLE`, `AEX_DUE_SCAN_SHARDS`, `AEX_LEASE_MS`, `AEX_STEP_DEADLINE_MS`, `AEX_MAX_ATTEMPTS` |
| `content-lifecycle-worker` | `AEX_MODE`, `AEX_CONTENT_TABLE`, `AEX_REGISTRY_TABLE`, `AEX_WORK_TABLE`, `AEX_CONTENT_BUCKET`, `AEX_CONTENT_BUCKET_OWNER`, `AEX_DENIAL_PROJECTION_TABLE`, `AEX_GC_STAGE_GRACE_HOURS`, `AEX_UPLOAD_GRACE_HOURS`, `AEX_DECLARED_CAPABILITIES`; plus `AEX_CONTENT_QUEUE_URL` and `AEX_CONTENT_DLQ_URL` in `delete` mode, `AEX_MARK_PAGE_ITEMS` and `AEX_SWEEP_PAGE_ITEMS` in `marksweep` mode, and optional `AEX_INVENTORY_BUCKET` in `reconcile` mode |
| `regional-secret-key-admin` | `AEX_SECRET_KEYSTORE_TABLE`, `AEX_KEYSTORE_LOGICAL_NAME`, `AEX_SECRET_KMS_KEY_ARN`, `AEX_ATTESTATION_OPERATION_ID` |

Forbidden bindings, which refuse the process when present:

| Deployable | Refuses |
| --- | --- |
| `regional-session-api` | `AEX_OPERATION_QUEUE_URL`, `AEX_CONTENT_QUEUE_URL`, `AEX_SECRET_KMS_KEY_ARN` |
| `regional-secret-api` | `AEX_SESSION_TABLE`, `AEX_CONTENT_TABLE`, `AEX_CONTENT_BUCKET`, `AEX_WORK_TABLE`, `AEX_REGISTRY_TABLE`, `AEX_OPERATION_QUEUE_URL` |
| `regional-stream` | `AEX_WORK_TABLE`, `AEX_OPERATION_QUEUE_URL`, `AEX_CONTENT_QUEUE_URL`, `AEX_SECRET_KMS_KEY_ARN` |
| `session-operation-worker` | `AEX_SECRET_KMS_KEY_ARN`, `AEX_CONTENT_QUEUE_URL` |
| `content-lifecycle-worker` | `AEX_SECRET_KMS_KEY_ARN`, `AEX_SESSION_TABLE` |
| `regional-secret-key-admin` | `AEX_SESSION_TABLE`, `AEX_WORK_TABLE`, `AEX_CONTENT_TABLE`, `AEX_REGISTRY_TABLE`, `AEX_SECRET_CUSTODY_TABLE`, `AEX_CONTENT_BUCKET` |

### Resource shapes

`release/units.toml` gains a `[unit.lambda]` block for the four Lambdas and a
`[unit.fargate]` block for the key-admin task; `regional-stream` already had one.
`graph verify` reports no `unit-resource-shape-missing` for any of the six.

### Gate note

`cargo fmt --all` **cannot run on this host**: `rustfmt` receives every member
file path on one command line and Windows refuses it with
`The filename or extension is too long. (os error 206)`. The equivalent gate was
run as `cargo fmt -p <package>` over each owned package, all exiting `0`. Linux
CI is unaffected; this is the same class of host condition as the NASM note in
the orchestrator conventions.

## Projection

Branch `rw/projection`, off `main`. This closes "The one blocking gap" above: a
regional authority value can now become an `aex_wire::models` value, and both
finite APIs mount routes.

### Where the projection lives, and why

`aex_regional_http::projection`. Three placements were possible and two are
wrong:

- **in `aex-session-app` or a domain crate** — a port that returned wire models
  makes the domain depend on the wire, which inverts the dependency direction the
  whole rewrite is built on;
- **in each deployable** — `regional:secrets` and `regional:provider-credentials`
  are each split across two deployables, so one representation would have two
  spellings and they would drift;
- **in `aex-regional-http`** — the crate that already owns the wire boundary for
  every regional deployable, and the only place both vocabularies may meet.

Its inputs are the **decoded authority rows** rather than a third view type. Any
other choice adds one type and one conversion per model, which is exactly the
drift a single projection exists to prevent (RS-28).

Two rules carry real weight and are asserted rather than described:

- **A secret's public `state` is read from the comparison the admission path
  uses, not from the stored `state` column.** An emergency revoke is one
  conditional update that raises `revokedThroughRevision` to the current revision
  and leaves the column alone — that is what makes it `O(1)` in the generation
  count — so a projection that copied the column would tell a customer `ready`
  about a record nothing may use.
- **An entity tag is derived from the projected representation**, canonicalized
  through the one JCS canonicalizer and domain-separated by model name. It
  therefore moves on a revoke, which advances no revision at all; a tag derived
  from a revision column would not have.

A tombstone has no public representation: it is `404`, never a `deleted` state on
the wire. A listing skips one rather than failing the whole page.

### What is mounted

| Deployable | Owns | Served today |
| --- | --- | --- |
| `regional-session-api` | 61 | **4** — `secret_get`, `secrets_list`, `provider_credential_get`, `provider_credentials_list` |
| `regional-secret-api` | 4 | **2** — `secret_delete`, `secret_revoke` |

`UnaryDispatch::served()` is the narrowing, derived by filtering the owned
partition rather than written as a second list. Every other owned route is
**absent from the router**, not mounted and answering a permanent failure
(RS-18), and `an_owned_but_unserved_route_is_absent_from_the_router` proves it on
both deployables.

Each served route is driven end to end in the `served` target: the router is
built by `mount_unary` over the real `UnaryDispatch`, a real HTTP request goes in,
and the published body, status and `ETag` come back. The two mutating routes
additionally assert the **expression** they committed, so a tombstone that
stopped conditioning on the observed revision would fail even though its body
would still be right.

### Idempotency without a receipt, where that is exact

`secret_revoke` and `provider_credential_revoke` declare an `Idempotency-Key` and
need no durable receipt. Their scope subject is the resource, their body is
`EmptyRequest`, and revocation is terminal, so one scope plus one key can only
ever carry one intent: `idempotency_conflict` is **unreachable** rather than
undetected, and a replay is answered from the stored row without a second write.
`a_replayed_revoke_answers_the_stored_receipt_and_writes_nothing` asserts both
halves.

`secret_delete` declares no `not_found`, which is deliberate: deleting an absent
or already-tombstoned name is a completed request. Both answer `204` **without a
write**, so a retry never advances the revision a concurrent editor is fencing
on.

### Adapter fields added

| Adapter | Field | Why it is persisted rather than derived |
| --- | --- | --- |
| `aex-secret-custody-dynamodb::ProviderCredential` | `fingerprint: ContentHash` | It cannot be recomputed without the plaintext, which no read path may hold. Minted once at registration by `SecretPlaintext::credential_fingerprint`: a domain-separated SHA-256 over length-prefixed `(workspace, credential)` then the value, so it is one-way and salted by the binding — the same key in another workspace does not fingerprint alike, and one precomputed table cannot cover the fleet. |
| | `name: ResourceName` | The human label the caller registered under. Nothing else stores it. |
| | `revision: u64` | A concurrency token a reader invented would be useless as one. |
| | `updated_at`, `revoked_at` | The wire publishes both; the row carried neither. |
| | `provider: ProviderId`, `state: CredentialState` | Were free `String`s. A stored value outside either closed set is now a decode failure rather than a value the projection has to guess at. |

`aex-secret-domain` gains `sha2` for the fingerprint. Nothing else changed there.

### Adapter behaviour added

- **`expressions::delete`** — the conditional custody delete `secret_delete`
  commits, which the previous pass recorded as missing. It **tombstones** the
  fence row rather than removing it: the row is what every use path conditions
  on, and deleting it would make a revoked-then-deleted name read as "never
  existed" instead of "deleted". Conditional three ways — the record must exist,
  be at the observed revision, and not already be a tombstone — and it clears the
  generation pointer so a later reader cannot resolve a name the customer
  believes is gone.
- **`expressions::revoke_provider_credential`** — one conditional update from
  `ready` only, the same shape as the secret revoke and for the same reason.
- **`expressions::set` now carries `REMOVE revokedAt`.** `aex_secret_domain::set`
  produces `revoked_at: None`, and the expression did not clear it, so a secret
  replaced after a revoke would publish a revocation date while being admissible
  again. This was a live divergence between a domain transition and the
  expression that commits it.
- **`store::load_provider_credential`** — a bounded point read. The sort key
  carries the provider, so an identity alone names a suffix; `ProviderId` is
  closed at six, so the read is six strongly consistent point reads and never a
  scan or an unbounded query.
- **`store::page_secrets` / `page_provider_credentials`** — listings that return
  the authority's own continuation. `list_secrets` could not name one, so a full
  page was indistinguishable from the end of a collection.
- **The receipt row codec moved to `aex_session_dynamodb::replay`**, which is
  always-on, so every regional table that holds a receipt stores the one shape.
  `codec::{encode_receipt, decode_receipt, receipt_is_live}` now delegate.

### Interface changes for peers

- `UnaryDispatch::dispatch` takes `&aex_regional_http::context::RequestContext`
  plus an `AcceptKind` instead of the wire context. The wire context carries no
  workspace for an `Account` principal, so a handler given only it could not
  scope an authority read at all. The implementor calls `cx.to_wire(accept)`.
- `RequestContext::now()` converts the edge receipt instant to `Timestamp` once,
  at the boundary, rather than in each handler.
- `CursorKeyRing::current()` names the key a new cursor is signed under.
  Rotation-overlap keys verify and never sign.
- `aex_regional_http::projection::authority_failure` is the one `StoreError` to
  `ErrorCode` mapping, exhaustive over the store vocabulary.

### What remains unmounted, and exactly why

| Route(s) | Blocker |
| --- | --- |
| `secret_put`, `provider_credential_register` | `SecretCrypto::seal` takes the **wrapped branch key** as an argument, and no port exposes it: `aex_secret_keystore_dynamodb::ActiveBranchKey` publishes `version`, `create_time`, `kms_arn` and `hierarchy_version` but not `BranchKeyRecord::enc`. Nothing can seal a plaintext until it does. `provider_credential_register` is blocked a second time: the request carries a human label plus plaintext but no workspace-secret binding, and OD-23 requires the `pcr_` row to reference one — which secret name a registration mints, and what happens when that name already exists, is decided by no accepted record, and the route declares no error code for the collision. |
| the 15 `sessions` routes | `aex-session-dynamodb`'s stored `SessionHead` cannot decode into `aex_session_domain::Session`: it holds no `initial_root`, `persisted_root`, `persist_revision`, `last_persisted_at`, `generation`, `work_admission`, `mutation_guard` or `lineage`, so `aex_session_app::ports::SessionReader` has nothing to build a snapshot from, and **no adapter implements any `aex-session-app` port** — a grep for `SessionReader` outside `aex-session-app` finds nothing. `session_get` and `sessions_list` are blocked twice over: the head stores only a `resolvedConfigDigest`, while `Session.resolvedConfig` and `SessionListItem.{model, provider}` need the resolved configuration itself. `session_create` is blocked a third time — `aex-session-app` declares `create_session` as an unwritten use case. |
| `session_message_send`, `session_messages_list` | **Contract gap.** `aex_wire::models::MessagePart` is `Text` or `File`; `aex_session_domain::MessagePart` is `Text`, `ToolCall` or `ToolResult`. The two vocabularies intersect only at `Text`, so neither direction is total: a wire `File` part has no domain arm and a domain tool part has no wire arm. No adapter work can close this. |
| the 21 `registry`, 6 `files`, 4 `uploads`, 3 `approvals`, 3 `operations`, 1 `usage` and 3 `workspace` routes | Their adapters exist but no projection was written for them in this pass. They are mechanically the same shape as the four served here — read the row, project, tag, page — and are unblocked. |
| `provider_credential_revoke` | Servable: the expression, the store method and the projection all exist. It is left unmounted only because the directory cannot be populated until `provider_credential_register` lands, so nothing would exercise it end to end. |

### The listener is still health-only

Neither `main.rs` calls `mount_unary`, and the reason is not the projection.
`mount_unary` requires an `EdgeAdmission`, whose only implementation
(`RegionalEdge`) needs an `AssertionSource` and a `KeyVerifier`:

- `AssertionSource` has no published `central-authz` request/response shape for a
  **workspace key** (`aex_internal_contracts::assertion` publishes
  `ResolveSessionForWorkspace`/`ResolvedSessionAssertion` for a browser session
  only) and no `aws-sdk-lambda` in `[workspace.dependencies]`;
- `KeyVerifier` has no parameter-store reader for `AEX_AUTHZ_VERIFY_KEYS_PARAM`
  and no `aws-sdk-ssm` in `[workspace.dependencies]`;
- `AEX_CURSOR_SIGNING_KEY_REF` resolves to a reference, and nothing turns it into
  the `CursorKeyRing` the listings sign continuations with.

All three are peer or infrastructure inputs, all three were already raised in
"Cross-stream requirements this pass raises", and none can be written here
without inventing a cross-plane contract. Mounting a route whose edge can admit
nothing would answer a permanent failure, which RS-18 forbids — so the handler
surface is complete and proved by `mount_unary` in the `served` targets, and the
listener keeps serving health until an edge can be built.

### Decisions taken beyond the sections above

| # | Decision | Rationale |
| --- | --- | --- |
| RS-28 | `aex-regional-http` depends on `aex-secret-custody-dynamodb`, `aex-secret-domain` and `aex-session-dynamodb`, and the projection's inputs are the decoded authority rows | The alternatives are a third view type per model (one extra type and one extra conversion each, both drift sites) or a copy of the projection in every deployable. Reversible: the dependency edge is the only cost, and the three other regional deployables link DynamoDB anyway. |
| RS-29 | An entity tag is a domain-separated SHA-256 over the JCS bytes of the **projected representation**, rendered quoted | Makes "the tag changed" and "the body changed" the same statement by construction, and gives one derivation for every `WithETag` route instead of one per resource. A revision-derived tag would not move on a revoke. |
| RS-30 | The public `SecretState` is derived from `revoked_through_revision < revision`, not from the stored `state` column | The column is deliberately untouched by the `O(1)` emergency revoke. Reading the fence is the only projection that agrees with the admission path. |
| RS-31 | `secret_revoke` and `provider_credential_revoke` honour their `Idempotency-Key` by terminal state rather than by a durable receipt | Their scope subject is the resource and their body is empty, so one scope plus one key carries exactly one intent. A receipt would add a row and a failure mode to make an unreachable conflict detectable. |
| RS-32 | `secret_delete` answers `204` without a write for an absent or already-tombstoned name | The route declares no `not_found`. A repeated delete that advanced the revision would break a concurrent editor's precondition for no reason. |
| RS-33 | The provider-credential fingerprint is salted by `(workspace, credential)` and minted once at registration | A bare digest of the key would let one precomputed table cover the fleet and would tell two customers they hold the same key. Per-binding salting removes both without making the value non-deterministic for its own binding. |

## Seams

Branch `rw/seams`, off `main`. This closes the section above: `mount_unary`'s
missing `EdgeAdmission` now has every input it needs, so **both finite APIs mount
routes on their real listeners**. The three blockers named in "The listener is
still health-only" are closed; a fourth — the projection reader — was
structurally required and is closed with them.

### What is served

| Deployable | Owns | Mounted before | Mounted now |
| --- | --- | --- | --- |
| `regional-session-api` | 61 | 0 (4 handler-complete, unmounted) | **9** |
| `regional-secret-api` | 4 | 0 (2 handler-complete, unmounted) | **2** |

`regional-session-api` adds the five registry listings — `registry_files_list`,
`registry_skills_list`, `registry_tools_list`, `registry_instructions_list` and
`registry_mcp_servers_list` — to the four custody reads it already answered.
RS-18 is unchanged: the other 52 owned routes are **absent from the router**, not
mounted answering a permanent failure.

### The workspace-key assertion exchange

`aex_internal_contracts::assertion` gains the pair `central-authz` had only for a
browser session:

```rust
pub struct CredentialDigest([u8; 32]);          // canonical unpadded base64url
pub struct AssertionSignature(Vec<u8>);         // 1..=512 bytes, base64url
pub const MAX_SIGNATURE_BYTES: usize = 512;
pub const MAX_KEY_ID_BYTES: usize = 128;

pub fn credential_bound_signing_input(
    assertion: &AuthorizationAssertion, credential_binding: &CredentialDigest,
) -> Vec<u8>;

pub struct ResolveWorkspaceKey {
    pub schema_version: SchemaVersion,
    pub key: ApiKeyId,
    pub presented_digest: CredentialDigest,
    pub region: Region,
    pub audience: AssertionAudience,
}

pub struct SignedAssertionEnvelope {
    pub schema_version: SchemaVersion,
    pub assertion: AuthorizationAssertion,
    pub key_id: String,
    pub credential_binding: CredentialDigest,
    pub signature: AssertionSignature,
}
```

**The key is named, never sent.** `aex_identity_domain::credential` stores
`HMAC-SHA256(pepper, SHA-256(token))` — a MAC over a *digest* rather than over the
token — and says in as many words that this exists so a regional edge can send
`{keyId, presentedDigest}` and keep the customer's plaintext regional. The request
is that design written down. A request carrying the token would authenticate
identically and would additionally place every customer key in the central plane's
logs, traces and memory.

The digest is `SHA-256` over the complete token, which is also exactly what
`PresentedCredential::binding` already computes. That is what makes the response's
`credentialBinding` comparable against the credential actually presented, and it
is compared twice: in `authz::signed_assertion` before the answer may enter the
cache, and again inside `assertion::verify`.

`credential_bound_signing_input` is the one definition of the covered bytes.
`SignedAssertion::verification_input` was a private second copy inside
`aex-regional-http`; it now delegates, so the issuer and every verifier cannot
disagree about what was signed.

### The four adapters

`aex_regional_http::authz`, one module, shared by every regional deployable:

| Type | Port it fills | Input |
| --- | --- | --- |
| `LambdaAssertionSource` | `AssertionSource` | a direct `lambda:Invoke` of `AEX_AUTHZ_FUNCTION_ARN` |
| `Ed25519Anchors` | `KeyVerifier` | the trust-anchor document at `AEX_AUTHZ_VERIFY_KEYS_PARAM` |
| `RegionalProjection<P>` | `ProjectionReader` | `aex_session_dynamodb::projection::AuthorizationProjection` |
| `ParameterStore` | — | resolves both documents, plus `AEX_CURSOR_SIGNING_KEY_REF` |

`central-authz` is IAM-invoked rather than routed, so the exchange is a direct
invoke: the caller's execution role *is* the authentication, and a role without
`lambda:InvokeFunction` on that one function ARN cannot resolve an assertion at
all. That is why `aws-sdk-lambda` and not an HTTP client.

Verification uses `ed25519-dalek`'s `verify_strict`, which rejects small-order
public keys and non-canonical signature encodings. The issuer signs with the same
library, so nothing legitimate is lost and the permissive-verifier class of bug is
unreachable rather than merely avoided.

Two documents, both read **once at cold start**, both `SecureString`-capable, and
neither with a fallback:

```json
{ "schemaVersion": 1,
  "keys": [ { "keyId": "2026-08-a", "publicKey": "<43 chars base64url>" } ] }

{ "schemaVersion": 1,
  "current": { "keyId": "2026-08", "material": "<32+ bytes base64url>" },
  "overlap": [ { "keyId": "2026-07", "material": "..." } ] }
```

Both parsers refuse an oversized, unversioned, unknown-membered, empty, over-long
or ambiguous document, and every refusal stops the process. A regional edge that
cannot verify an assertion must not start: the alternatives are a listener that
accepts nothing while reporting ready, or one that accepts everything.

### The edge gained a placement stage

A credential names its own API key and nothing else — the workspace it belongs to
is a fact only the assertion carries — and the `regional-authz-projection` is
keyed the way its writer keys it: revocation by `KEY#`, placement by `WS#`. The
two facts therefore become available at two different points, and
`ProjectionReader` has two methods rather than one:

```rust
async fn project(&self, credential: &PresentedCredential) -> Result<ProjectedEpochs, ProjectionError>;
async fn placement(&self, workspace: WorkspaceId) -> Result<ProjectedState, ProjectionError>;
```

`ProjectedState` gains `region`. The admission order is now: credential, the key
revocation floor, assertion verification against that floor, the workspace the
assertion names, the placement, the region and epoch and account-state gates, the
declared scope, the pause gate, the effective body bound, and replay identity.

Both projection reads happen on **every** request and neither is cached, which is
the only thing that makes a 30-second assertion cache safe.
`the_placement_is_read_on_every_request_even_when_the_assertion_is_cached` asserts
it directly.

### Decisions taken beyond the sections above

| # | Decision | Rationale |
| --- | --- | --- |
| SD-01 | The `central-authz` request names the key by `(keyId, presentedDigest)` and never carries the token | The stored verifier is a MAC over the digest precisely so the plaintext can stay regional. Sending the token authenticates identically and additionally puts every customer key in the central plane. |
| SD-02 | `AEX_CURSOR_SIGNING_KEY_REF` resolves to a Parameter Store name holding a `SecureString` ring document | The variable was a "storage reference" with no resolver at all. Parameter Store is already the plane's answer for the trust anchors, so this adds a document rather than a mechanism, and `with_decryption` is asked for unconditionally so there is one code path rather than two. |
| SD-03 | `ProjectionReader` splits into `project` (credential) and `placement` (workspace) | Nothing writes a credential-to-workspace index, and inventing one would be inventing a projection row. Splitting the trait reads what the table actually holds, at the two points each fact becomes available. |
| SD-04 | A `deleting` placement projects to `paused` rather than to a refusal | The pause-exempt set is exactly what a deleting workspace still needs, trash and purge, and every route that starts new paid work is outside it. |
| SD-05 | `Dispatcher` moves out of each `served` target and into each deployable's library | It was written once per test and would have been written again in each `main`. Two spellings of the composition is how a tested router and a mounted router drift apart. |
| SD-06 | The registry projection is one macro over five identical field sets, discriminated by `RegistryKind` | The five wire models are distinct structs with identical fields; the only variation is the type name, and hand-writing that variation is hand-writing the drift. `WrongRegistryKind` makes projecting a skill as a tool a typed refusal rather than a published lie. |
| SD-07 | A registry listing binds its cursor to a per-registry snapshot token | All five share one table and one key template, so nothing else stops a `skills` cursor resuming a `tools` read. The binding is authenticated, so a re-pointed cursor fails its MAC. |
| SD-08 | `regional-secret-api::Config::limits` carries zero page bounds | It owns no listing. Zero is not a page size it would ever use, so a handler that started paginating fails its own budget check rather than inheriting a number nobody chose; `a_served_route_never_paginates` holds the premise. |

### Fixed on the way

`aex-session-dynamodb` gated `wire_pending` on `session-authority` alone, so
`default-features = false, features = ["authz-projection"]` — the exact
composition D-21 exists to permit — did not compile. Nothing noticed, because the
default feature set turns both on. The module now follows either feature.

### What the remaining 52 routes are waiting for

This corrects the previous pass, which recorded the registry, files, uploads,
approvals, operations, usage and workspace routes as "mechanically the same shape
and unblocked". They are not. Only `aex-secret-custody-dynamodb`,
`aex-registry-dynamodb`, `aex-content-dynamodb` and `aex-work-dynamodb` publish a
store trait at all, and of those only the first two publish a read a listing can
use; `aex-usage-query-aws` publishes expression builders and no store type.

| Route(s) | Precise next blocker | Owner |
| --- | --- | --- |
| the 5 registry `*_get` and 5 `*_put` | A registry pointer stores `sha256` and a size; the item form of every registry model carries the `value` itself, which is a **sealed** body in content storage. Serving one needs the content data key and a decrypt path, neither of which `regional-session-api` composes. A listing is complete without it — the wire model marks `value` "omitted in collection rows" — which is exactly why the 5 listings are served and these 10 are not. | regional services + regional stores |
| `registry_files_download_create` | Nothing presigns. `aex-content-dynamodb::encode_grant` exists; no composed path mints a grant. | regional stores |
| the 3 `operations` | `aex_work_dynamodb::WorkAuthority` publishes `load` and `scan_due` and **no workspace-scoped listing**, so `regional_operations_list` has no query to run. `WorkRecord` additionally carries no operation result and no `ApiErrorBody`, and its `kind` is a free `String` rather than `OperationKind`, so `Operation.result`, `.error` and `.kind` have no source. | regional stores |
| the 3 `approvals` | `aex_session_dynamodb::wire_pending::Approval` carries neither `session_id` nor `expires_at`, both of which `models::Approval` requires, and no store method reads one. | regional stores |
| the 6 `files` | Every template is `/api/sessions/{sessionId}/files/...`, so each needs the session's persisted root — the `SessionHead`-cannot-decode blocker already recorded above. A `TreePage` is additionally a **sealed** body, so listing entries needs the content data key too. | regional domains + regional stores |
| the 4 `uploads` | `RegistryStore::load_upload` reads one, but `models::Upload` publishes the presigned target and nothing presigns. | regional stores |
| the 1 `usage` | `aex-usage-query-aws` publishes `expressions::aggregate_page` and no store type at all — there is nothing holding a client to call it. | usage metering |
| the 3 `workspace` | `models::Workspace` requires `apiUrl`, `name`, `slug`, `createdAt` and `operationalState`; the `regional-authz-projection` placement row carries none of them. `EffectiveWorkspaceLimit` has no regional source at all. | central identity/control, the projection's only writer |
| the 15 `sessions`, `session_message_send`, `session_messages_list`, `secret_put`, `provider_credential_register` | Unchanged from the previous pass. | as recorded above |

### Peer work this pass raises

| Requirement | Owner |
| --- | --- |
| `central-authz` must serve `ResolveWorkspaceKey` on an invoke handler. `run()` mounts only the health router today, and its `issue_for_key` returns `aex_identity_domain::assertion::Assertion` — a 323-byte binary envelope — rather than the `SignedAssertionEnvelope` that `aex-regional-http`, `regional-observation-api` and `regional-otlp` all verify. **Two assertion vocabularies exist and neither is served.** One has to go. This pass deliberately did not pick for the identity stream: it implemented against the one three regional deployables already verify, and records the divergence here. | central identity |
| `ResolvedSessionAssertion` carries a claim set and nothing that authenticates it, so no regional edge can verify a browser-session assertion. It should answer with `SignedAssertionEnvelope`. | contracts + central identity |
| `regional-observation-api::edge` and `regional-otlp::edge` each hold a private `Ed25519Anchors`, an `AssertionResponse` and an `HttpAssertionSource` that posts to `/internal/authz/assertions` — a third spelling of this exchange, over a transport `central-authz` does not expose, and one that sends the credential verbatim. Both should adopt `aex_regional_http::authz`. | regional services, next pass |
| `central-control-worker` must confirm the `regional-authz-projection` item shapes, and must publish whatever `models::Workspace` needs before the three regional `workspace` routes can be served. | central identity/control |

## Assertion

Branch `rw/assertion`, off `main`. This closes the three items the "Peer work
this pass raises" table above assigned to central identity and to "regional
services, next pass". The full record is
`references/rewrite/central-identity.md` §10; what changed here:

- **The wire form is the 323-byte binary envelope.** The JSON
  `AuthorizationAssertion`, both signing inputs, `AssertionSignature` and
  `SignedAssertionEnvelope` are deleted from `aex-internal-contracts`. The
  contract crate now owns the *exchange* — `AssertionAudience`,
  `CredentialDigest`, the two requests, `IssuedAssertion` and
  `AssertionResponse` — and `aex-identity-domain` owns the artifact.
- **`aex-regional-http` no longer verifies anything itself.** Its local `verify`,
  `SignedAssertion`, `Ed25519Anchors` and the `KeyVerifier` port are gone;
  `RegionalEdge` loses its `V` type parameter because the `VerificationKeySet` is
  a concrete input rather than a port. `SD-01` survives unchanged: the request
  names the key by `(keyId, presentedDigest)`.
- **`ProjectedEpochs` gains subjects.** It is now
  `{ key, workspace, account }` bound to their ids through `RegionalFloors`, and
  `ProjectedState` carries the organization the account epoch belongs to. A
  subject kind this region does not project answers a floor no assertion can
  satisfy, so a person's envelope is refused rather than admitted.
- **`EdgeBinding` gains `plane`**, and `config::plane_name` resolves to the typed
  `Plane` rather than a validated `String`.
- **`regional-observation-api::edge` and `regional-otlp::edge` are deleted.** Both
  now compose `aex_regional_http::authz` and admit through `RegionalEdge`. Their
  `AEX_CENTRAL_AUTHZ_URL` and `AEX_ASSERTION_TRUST_ANCHORS` become
  `AEX_AUTHZ_FUNCTION_ARN` and `AEX_AUTHZ_VERIFY_KEYS_PARAM`, matching the two
  finite APIs, and both gain `AEX_AUTHZ_PROJECTION_TABLE` because they now
  actually read the projection — previously they passed
  `ProjectedEpochs::default()`, so a revoked key kept working for the full
  assertion lifetime and a paused account was never gated.

The trust-anchor document a deployable reads at cold start changes shape: its
`keyId` is the envelope's raw-UUID `kid` rather than a free string, and each
entry carries `notAfterMs`.
## Reads

Branch `rw/regional-reads`, off `main`. This pass was scoped to the adapter reads
the 52 unmounted routes were recorded as waiting on. It closed the two adapter
gaps it found, mounted one more route, and **corrects the blocker table above**:
for four of the six remaining families the adapter is not the blocker, and no
amount of store work would have mounted them.

### What is served

| Deployable | Owns | Mounted before | Mounted now |
| --- | --- | --- | --- |
| `regional-session-api` | 61 | 9 | **10** |
| `regional-secret-api` | 4 | 2 | 2 |

The addition is `provider_credential_revoke`.

### `provider_credential_revoke` was mounted, and it was not servable as written

The previous pass recorded it as "servable ... left unmounted only because the
directory cannot be populated". That reason does not hold: `provider_credential_get`
and `provider_credentials_list` are already mounted over the same unpopulated
directory, and a read over rows nothing yet writes answers `404` and an empty
page — both honest.

The route *was* unservable, for a different reason nobody had recorded.
`regional-session-api` owns it, and `migrations/regional/tables/regional-secret-custody.json`
grants that role `GetItem`, `Query` and `TransactWriteItems` — **not `UpdateItem`**.
`expressions::revoke_provider_credential` returns an `UpdateBuilder`, and
`SecretCustodyStore::commit_update` issues it as a bare conditional update, which
that role is denied. Issued that way the route would pass every local test and
fail in production with `AccessDeniedException`.

The handler therefore wraps the same expression in a one-action
`TransactionPlan` and commits it through `SecretCustodyStore::commit`. That is
the write path the role actually holds, and it keeps the "no unconditional
authority write" check on the path. `the_revocation_reaches_the_authority_as_a_transaction`
asserts it, and `commit_update` stays a typed refusal in the fixture so a
regression to the denied path fails the suite rather than the deployment.

Idempotency is by terminal state (RS-31): a replay observes `revoked`, answers
from the stored row and writes nothing. The transaction's deduplication token is
derived from the binding plus the **observed** revision, so two attempts against
the same observed state are one transaction and an attempt against a moved row is
not.

### `aex-usage-query-aws` has a store type

`UsageProjectionReads` publishes the three reads the projection holds — the
generation pointer, one coverage row and one bounded page of rollups — and
`UsageQueryStore` is its DynamoDB adapter. Decoding is strict against the exact
attribute names `aex_usage_application::projection` writes.

Three decisions worth naming:

- **It stays inside its own stream.** The three usage authority adapters carry
  their own row reader and their own port error rather than linking
  `aex-session-dynamodb`, so this one does too. That is what keeps
  `tests/write_incapability.rs` a fact about this crate's sources alone.
- **`current_generation` answers `Option`.** An absent pointer means the
  projection was never cut over. Substituting `Generation::FIRST` would make a
  never-built projection read as an empty one.
- **Every read is strongly consistent, including the page query.** The coverage
  vector exists to distinguish "you used nothing" from "we have not folded your
  facts yet"; a coverage row from a replica could name a frontier ahead of the
  rows the same request returned.

`QueryError` gains `Unavailable`, `Denied` and `Misconfigured`. There is no
ambiguous-commit arm, because every call the crate can make is a read.

### `session-authority` has an approval codec and a read port

The `approval` item type was declared in the table definition and in
`keys::ITEM_TYPES`, and `keys::approval` built its key, but **nothing encoded or
decoded one and no store method read one**. The placeholder row carried five
loose strings, no session, no workspace and no expiry.

`wire_pending::Approval` now mirrors `aex_session_domain::approval::Approval`:
one `ApprovalBinding` of all eleven bound fields, a five-arm `ApprovalStatus`, a
seven-arm `ApprovalCancelCause`, the workspace the tenancy check compares, and
`expires_at`. All eleven bound fields are persisted rather than the seven the
wire publishes, because `respond` revalidates the whole binding and an approval
that cannot be revalidated would have to be dispatched on trust.

`SessionReads` is the read-only adapter: a client and a table name, no cursor key
because it mints no token and no transaction compiler because it commits nothing.
`SessionQueries` publishes the head read plus one approval and a bounded page of
them, listed by a `begins_with` range inside the session partition so an approval
of another session is unreachable rather than filtered out.

The point read and listing are mounted. The response route remains absent until
the write-side decision adapter can revalidate all eleven fields and commit the
winner atomically.

### The blocker table, corrected

The previous pass attributed the remaining families to the stores. Four of them
are blocked in the contract or the domain instead, and the evidence is in the
generated models rather than in the adapters.

| Route(s) | Precise next blocker | Owner |
| --- | --- | --- |
| the 1 `usage` | The public regional model now reports the quantities the fold actually produces. Monetary rating remains on central finance surfaces backed by private rate books; `publishedSequence` / `projectedSequence` match the domain frontier and `serviceThrough` is optional. The remaining blocker is a query planner that implements the full multi-category, time-range, grouping and continuation contract rather than exposing the store's one-row primitive. | usage application + regional services |
| the 3 `approvals` | `cancelled` and `expired` are distinct reachable states and every approval carries a caller-supplied future deadline. `GET` and list are served through strongly consistent reads and a session-bound cursor. Only response remains blocked on the atomic write/revalidation adapter. | regional services write path |
| the 3 `operations` | The domain and row codec now preserve phase, exact typed result payload, durable failure, lifecycle timestamps and `WorkspaceDelete`; public projection parses payload under the authoritative envelope kind. `ContentGc` is excluded from both point projection and the sparse public index. The remaining blocker is a `SessionQueries` point/list adapter with complete filter and pagination semantics; no operation route is mounted yet. | regional stores + regional services |
| the 3 `workspace` | `AEX_REGIONAL_API_URL` is validated configuration. Profile and effective-limit records have separate, typed cold projection rows and a read-only port, keeping placement narrow. Routes remain absent because `central-control-worker` does not yet write those rows and the verified assertion still lacks the complete `AccountOperationalState` payload (`changedAt`, revision and paused details). No reader invents defaults or pause facts. | central identity/control producer |
| the 10 registry `*_get`/`*_put`, 6 `files`, 4 `uploads` | Unchanged: the content decrypt path, the session's persisted root, and presigning. | as recorded above |

### Where the missing `Workspace` fields belong

The question the `workspace` family raises is whether `name`, `slug`,
`createdAt` and `apiUrl` should join the `regional-authz-projection` placement
row. They should not, and the reason is what the placement row is for.

The placement row is an **admission** projection: region, status and three
epochs. `the_placement_is_read_on_every_request_even_when_the_assertion_is_cached`
means it is read on *every* request and is never cached — that is the property
that makes a 30-second assertion cache safe. Widening a row on the hot path with
four descriptive attributes used by three routes pays a per-request cost for data
almost no request wants, and it turns a display-name edit into a write on the
authorization path.

The recommended split, for `central-control-worker` to confirm:

- `name`, `slug` and `createdAt` belong on a **separate `workspace_profile` item**
  in `regional-authz-projection`, written by the same control feed and read only
  by the three `workspace` routes. Same table, same writer, same residency; the
  hot row stays small.
- `apiUrl` is **not a per-workspace fact at all**. It is one value per plane and
  region, so it belongs in the deployable's configuration
  (`AEX_REGIONAL_API_URL`) beside `AEX_REGION`, not repeated on every workspace
  row where it could disagree with the host that served the request.
- `operationalState` needs a richer signed source. The current assertion carries
  only active/paused plus epochs; it cannot produce the wire state's
  `changedAt`, revision, pause reason or optional restoration/deletion facts.
- `status`, `region`, `id` and `organizationId` are already on the placement row.

### Decisions taken beyond the sections above

| # | Decision | Rationale |
| --- | --- | --- |
| RD-01 | A revocation `regional-session-api` owns is committed as a one-action `TransactWriteItems`, never as a bare conditional update | The role is granted the former and denied the latter. A capability the code assumes and IAM refuses is the class of defect that only appears in production, and the transaction path additionally keeps the unconditional-write check on the path. |
| RD-02 | `aex-usage-query-aws` keeps its own row reader and error vocabulary rather than linking `aex-session-dynamodb` | The three usage authority adapters already do, and the write-incapability proof is a scan of this crate's own sources plus its own manifest. Borrowing another stream's reader would make "read-only" a claim about a dependency instead of a fact about the crate. |
| RD-03 | The approval row persists all eleven bound fields, not the seven the wire publishes | `respond` revalidates the whole binding and commits `Cancelled { BindingDrift }` when any of the eleven moved. A row holding the public subset could not perform that comparison, so the bound call would have to be dispatched on trust. |
| RD-04 | The workspace profile fields land on a second projection item rather than on the placement row | The placement row is read on every request and never cached. Descriptive data belongs beside the hot row, not inside it. |
| RD-05 | This continuation mounts exactly the two fully served approval reads | The response route still needs an atomic writer; usage needs the full query planner; operations need point/list reads; workspace needs central producers and a richer signed account-state fact. RS-18 keeps all four absent. |
| RD-06 | Regional resource usage contains no monetary amount | Real rate books remain private in central finance. Publishing quantities from the regional fold is authoritative; copying private rates or guessing money at the edge is not. |
| RD-07 | Approval expiry is explicit input, not a hidden default | The policy owner supplies a future deadline. The domain refuses an absent window and turns a response racing the deadline into an `Expired` commit. |
| RD-08 | Public operation payloads are decoded under the envelope kind | Stored content has no second discriminant. A mismatch is typed corruption, while internal `ContentGc` never enters the public index or point result. |
| RD-09 | Workspace profile and limits use cold rows and a separate port | Placement stays the narrow per-request authorization item. Effective values are durable feed records, including their source; generated registry metadata is never treated as a value. |
