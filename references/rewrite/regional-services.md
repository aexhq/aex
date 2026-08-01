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
last_verified: 2026-08-01
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
