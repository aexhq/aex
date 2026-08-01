---
title: Central identity, control and authorization — stream handoff
description: What the central identity/control stream implemented on rw/central-identity, what it deliberately left undone, the exact types it publishes, what it needs from peers, and every decision it took beyond the orchestrator conventions.
keywords:
  - identity
  - authorization
  - control plane
  - aurora
  - assertion
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-01
related:
  - references/rewrite/contracts.md
  - references/rewrite/test-architecture.md
---

# Central identity, control and authorization — stream handoff

Plan of record: `references/rust-native-rewrite-2026-07-31/plans/02-central-identity-control.md` in the parent workspace.

Branch `rw/central-identity`. Nothing is pushed. The continuation starts from
`main` at `54c2d572` and has four logical implementation commits, each green at
the point it was made: `f76d116e`, `9747a218`, `3cbbe1b7` and `9e76e887`.

## 1. What is implemented

### `aex-rds-data` — the one Aurora Data API transport

The whole surface the platform needs from the Data API, and nothing more.

- `SqlValue` has **no `f64` arm** and `Record` has **no `f64` accessor**. A
  `doubleValue` arriving in any column, including inside an array, is
  `DecodeError::UnexpectedDoubleValue` unconditionally. Floating point cannot
  cross this boundary in any schema, not merely in the money schema.
- The 768 KiB result and 56 KiB field budgets are applied **before** any record
  is decoded, so a paging bug is `ResultTooLarge` rather than a truncated read.
- The full failure table: `40001` and `40P01` as separate arms, `23505`/`23503`/
  `23514`/`23502`/`23000` keeping the exact constraint name, `42501`,
  `42P01`/`42703`, every named service exception, and the
  `resuming|auto-paused|not currently available` message form. Unmatched is
  `Fatal` and never retried.
- `commit()` consumes `self` and answers `CommitFailure::Unknown` for every lost
  response. `RolledBack` is produced only when the service **states** the
  transaction aborted — a reported rollback status, or a `23xxx`/`40001`/`40P01`
  raised at `COMMIT` by a deferred constraint trigger, where PostgreSQL has
  definitively rolled back. The crate carries no retry policy at all.
- Every in-transaction call takes `&mut self`, so serial use of one transaction
  id is a compile-time property. Dropping a transaction without a terminal call
  emits `transaction_leaked`.

The transport seam is a `Transport` trait, so the budget, deadline and ambiguity
rules are provable without an endpoint — and it is what lets the repositories
count statements.

### `aex-control-domain` — one scope vocabulary, one role, one decision

- `ScopeSet` is a `u64` bitset over the generated 28-entry `aex_wire::ScopeId`
  registry. There is no second registry: the assertion envelope's `scopes` field
  is that bitset, and a bit outside the registry cannot be rebuilt.
- `OrgRole { Member, Admin, Owner }`, ordered, with `scopes()` per role.
  `ADMIN` is `OWNER` minus exactly `workspaces:delete`, asserted.
- The authorization matrix is derived: `scope`, `pause_exempt` and the admitted
  principal kinds come from the **generated** `RouteDescriptor`, and a 27-row
  rule table adds the one fact the wire contract does not carry — the role
  floor. A test asserts the rule table covers exactly the central route table.
- Two invariants are theorems rather than intentions:
  *every route that is not pause-exempt resolves an organization*, and *a
  workspace key can never be redirected at another organization or workspace*.
- `Epoch` has `advance()` and no decrement constructor.
- `canonical_intent_hash` is `aex_wire::canonical` plus one rule: any
  floating-point number anywhere in the body is `FloatingPointForbidden`, with
  the RFC 6901 pointer that names it. Fields are length-prefixed, so a boundary
  cannot be shifted between two adjacent identity fields.
- Cursors are `cur_<payload>.<mac>`, fixed-layout, constant-time-checked, 24-hour
  expiry, and every bound field is compared — MAC first, so a forged cursor
  learns nothing.

### `aex-identity-domain` — the credential codec and the 323-byte assertion

- Five credentials, one primitive, one codec. The stored value is
  `HMAC-SHA256(pepper_v, SHA-256(complete token))`, so a regional edge transmits
  `{keyId, presentedDigest}` and the customer's plaintext never crosses a region
  boundary. `parse` is structural only and never compares a secret.
- Canonical unpadded base64url with a constrained final character, and Crockford
  base32 whose leading character is at most `'7'`, so one byte string has exactly
  one textual spelling.
- The assertion is a fixed-layout **323-byte** envelope signed Ed25519, with a
  hex golden for a fixed claim set, a single-bit mutation sweep over all 323
  bytes, and an exact thirty-second boundary (accepted at expiry−1 ms, refused at
  expiry). `verify` re-checks the lifetime and refuses a validly signed assertion
  that claims longer, so an issuer bug cannot lengthen the window.
- Device user codes are `XXXXX-XXXXX` over a 20-symbol unambiguous alphabet
  (~43.2 bits) by **rejection sampling**; a test sweeps the 240 accepted byte
  values and asserts every symbol is chosen exactly twelve times.
- Every secret-carrying type renders `<redacted:N bytes>`, asserted over the
  minted token, the digest, the pepper, the verifier, the cursor secret, the
  signer and the email address.

### The application layer

Coarse ports: one method equals one atomic unit of work, and no unit-of-work
handle crosses the boundary. `TxOutcome::Unknown` becomes a typed retryable
error naming the **preassigned** identity; nothing retries under a fresh one.

Workspace provisioning is three explicit steps, and the whole eight-row
unknown-outcome matrix is asserted on durable facts rather than on status codes:
an unknown or unavailable region leaves the workspace hidden and the same
`Idempotency-Key` retryable, a lost `T2` commit is pending rather than a false
`201`, and a region answering about a *different* workspace is refused outright.

### The DDL, `migrations/central/20260801000000_bootstrap.sql` through `20260801000300_control_functions.sql`

- `REVOKE ALL ON DATABASE` and `DROP SCHEMA public CASCADE`; no extensions.
- Group roles are `NOLOGIN`; grants are an **allowlist with no denylist**, so
  they cannot rot against a dropped table the way the previous `REVOKE` list did.
- Epochs are writable only through five per-kind `SECURITY DEFINER` wrappers, and
  `control.bump_epoch(text, uuid)` is revoked from `PUBLIC`. No application role
  holds `UPDATE` or `DELETE` on `control.authorization_epoch` or
  `control.audit_event`.
- `membership_owner_required` is `DEFERRABLE INITIALLY DEFERRED`, so two
  transactions racing to remove the last two owners cannot both win.
- `control.invitation` has **no token, hash, verifier or secret column at all**.
- `identity.external_identity` has no access-, refresh- or id-token column.

### The Aurora adapters

- The five pinned authorization statements, including
  `resolve_session_for_workspace`, which is byte-for-byte the account-token
  statement apart from the credential table. One credential path, not two.
- The pinned I/O budget is **counted**: resolving a key or a session is exactly
  one statement and zero transactions, asserted against a counting stub.
- `AuroraIdentityStore` implements all fourteen `IdentityStore` units of work.
  Credential verification failures roll back open transactions, single-use
  guards live in the write predicate, and a lost commit returns the preassigned
  reconciliation identity rather than minting another credential.
- `AuroraControlStore` implements all twenty-two `ControlStore` units of work:
  replay identity, organization/finance creation, invitations, fenced workspace
  provisioning and deletion, atomic API-key revocation plus epoch advance,
  operation/outbox claims and bounded GC. Replay attachment/completion and
  operation fences require exactly one affected row before commit.
- The two central actor reads are now one bounded statement each. Active
  memberships are a JSON array of fixed three-field tuples, capped at 1,001 so
  the row decoder can fail closed above the public 1,000-row bound. A dashboard
  session receives only the generated bootstrap route's `account:read` scope.
- Source discipline is asserted rather than asked for: no format placeholder, no
  positional parameter, no bare `timestamptz` projection, every millisecond
  parameter through the cast, every ordered read bounded, every single-use
  consumption carrying its guard in the `WHERE` clause, and no credential ever
  looked up by its verifier.
- `map_store_error` keeps the exact constraint name, maps both constraint
  triggers from their message, and maps a lost transaction to `Unknown` rather
  than `Unavailable` — the difference is whether a credential may already exist.

### The migration suite

`crates/aex-control-aurora/tests/migrations.rs`, `required-features =
["integration-engines"]`, sixteen cases against real PostgreSQL. It proves the
bundle applies, that `public` is gone, that no provider-token or invitation-secret
column exists, that the two constraint triggers fire, that only one signing key
can be active, that an epoch advances only through its wrapper and that
`control.bump_epoch` is unreachable — and, most importantly, the **role-denial
matrix**: `aex_authz` holds no write privilege on any table in either schema, its
write probe really fails, and the five authorization statements `PREPARE`
successfully as that role.

## 2. What I deliberately left undone

Each is a tracked gap with a named blocker, not an oversight.

| Gap | Why, and what it costs |
| --- | --- |
| **`aex-central-http`.** Still the skeleton from `main`. | A fresh feasibility check at `3cbbe1b7` found that `aex_wire::server` still publishes only request/response shapes. There are no generated fragment server traits or mount functions, and contracts handoff §2 explicitly records the omitted eighteen traits. Binding handlers against `ROUTES` by hand would be a second route table. |
| **The four deployables' bodies.** Their config parsing, startup denial and unit tests are the scaffolds from `main`; their Lambda shapes are declared. | `central-identity-api`, `central-control-api`, `central-authz` and `central-control-worker` still return `NotImplemented`. The first three cannot mount the absent generated server surface, and composing only the worker while its public peers remain unmountable would not produce a runnable central plane. |
| **Continuation cursors in `AuroraControlStore`.** First pages are bounded and work; a non-empty opaque cursor fails closed with `StoreError::Decode`. | `PageRequest` carries only the opaque string, while the adapter needs authenticated `(created_at, id)` claims and has no cursor secret. The HTTP/application boundary must decode the signed cursor and pass typed keyset fields; silently ignoring or locally decoding an unauthenticated string would be wrong. |
| **The four live companions.** Untouched. | `OD-07`: nothing is deployed or credentialed in this run, so no live receipt is earnable. |
| **A separate `authorization-scopes.v1.json` under `api/schemas/`.** Not created. | The contracts stream already landed the 28-scope registry at `api/schemas/registries/scopes.yaml`, generated into `aex_wire::ScopeId`. Creating a second scope file would be exactly the drift this stream exists to remove. Decision D-27 below. |

## 3. Exact types published

```rust
// The sole assertion implementation. Pure; links no AWS SDK.
use aex_identity_domain::assertion::{
    verify, issue, signing_bytes,
    Assertion, AssertionClaims, AssertionSigner, LocalSigner, Audience, Plane,
    RegionalService, PrincipalKind, AssertedAccountState, EpochSlot, EpochSlots,
    EpochProjection, VerificationInputs, VerificationKey, VerificationKeySet, KeyId,
    IssueError, VerifyError, KeySetError,
    ASSERTION_ENVELOPE_LEN, ASSERTION_BODY_LEN, ASSERTION_SIGNED_LEN,
    ASSERTION_MAX_LIFETIME_MS, ASSERTION_MAGIC, ASSERTION_VERSION,
    ASSERTION_ALG_ED25519, EPOCH_SLOTS, MAX_VERIFICATION_KEYS,
    workspace_key_binding, account_token_binding, user_session_binding,
};

// The credential codec, for the regional edge and `aex-cli`.
use aex_identity_domain::credential::{
    parse, mint, verifier, verify, credential_binding, encode_id, decode_id,
    CredentialKind, CredentialError, ParsedCredential, PresentedDigest, Verifier,
    Pepper, PepperVersion, MintedSecret, RegionCode, SecretRng,
    SECRET_BYTES, SECRET_TEXT_LEN, ID_LEN,
};

// The identity state machines.
use aex_identity_domain::{
    User, UserStatus, UserTransition, NormalizedEmail, EmailError,
    ExternalIdentity, Provider, ProviderAccountId, UnlinkDenied, may_unlink,
    DashboardSession, SessionState, SessionTransition, DASHBOARD_SESSION_TTL,
    EmailChallenge, ChallengeState, ChallengeTransition, EMAIL_CHALLENGE_TTL,
    DeviceAuthorization, DeviceState, DeviceTransition, PollDecision, UserCode,
    UserCodeError, DEVICE_TTL, DEVICE_POLL_INTERVAL, DEVICE_SLOW_DOWN_STEP,
    AccountToken, TokenOrigin, TokenTransition, ACCOUNT_TOKEN_TTL,
};

// The authorization decision and the scope/role vocabulary.
use aex_control_domain::{
    Scope, ScopeSet, ScopeError, OrgRole, PrincipalKindTag, PrincipalKinds,
    Principal, ActorCredential, OrgMembership, Action, Resource, ResourceClass,
    Requirement, Granted, Denial, Admission, AccountState, NotCentral,
    decide, admit, requirement,
    Epoch, EpochSubjectKind,
    IntentHash, IdempotencyIdentity, IdempotencyKeyKind, ScopeKind, IntentError,
    canonical_intent_hash,
    CursorClaims, CursorSecret, CursorError, encode_cursor, decode_cursor,
    base64url, unbase64url,
    Organization, OrganizationStatus, Membership, MembershipStatus,
    MembershipTransition, Invitation, InvitationStatus, InvitationTransition,
    Workspace, WorkspaceStatus, WorkspaceTransition, ApiKey, ApiKeyTransition,
    Operation, OperationKind, OperationStatus, OperationVisibility,
    OperationTransition, Fence, Lease, LeaseOwner,
    AuditEvent, ActorKind, AuditOutcome, ResourceKind, OutboxMessage, Topic,
    Slug, SlugError, Revision, RevisionError,
};

// The transport, for every central request and worker Lambda including finance.
use aex_rds_data::{
    DataApiClient, DataApiConfig, ResourceArn, SecretArn, DatabaseName, ConfigError,
    Statement, SqlValue, sql, Record, Row, Transaction, TransactionId, Isolation,
    Committed, CommitFailure, DataApiError, DecodeError, SqlState, ExceptionKind,
    Transport, TransportError, ExecuteResponse, AwsTransport,
};

// The ports peers implement or consume.
use aex_identity_app::ports::{IdentityStore, PepperKeystore, Clock, IdFactory,
    TxOutcome, UnknownCommit, ReconcileIdentity, StoreError, RequestContext};
use aex_control_app::ports::{ControlStore, AuthorizationReader, RegionalControlPort,
    MailerPort, EffectError, Page, PageRequest, MAX_PAGE_LIMIT,
    WorkspaceKeyState, AccountActorState, CentralActorState, SigningKeyRecord,
    ProvisionWorkspaceRequest, ProvisionWorkspaceResponse,
    DeleteWorkspaceRequest, DeleteWorkspaceResponse, InvitationEmail};
use aex_control_aurora::{AuroraAuthorizationReader, AuroraControlStore,
    CentralActorRow, sql as control_sql, map_store_error, map_commit_failure};
use aex_identity_aurora::{AuroraIdentityStore, sql as identity_sql};
```

Also published for the finance stream: `control.bump_account_epoch(uuid)` — call
it in the **same transaction** as every pause and resume, or revocation is
bounded only by the thirty-second expiry.

## 4. What I need from a peer

| `TODO(cross-stream)` | Owner |
| --- | --- |
| `aex-test-harness` must expose a container builder (e.g. `containers::postgres() -> GenericImage`). The `data-image-literal` rule bans the container library's only constructor in every path under `/tests/` and exempts one directory — the harness — so **no product crate can start a container at all** today. My migration suite therefore takes a lane-supplied database through `required_env!("AEX_CENTRAL_PG_URL")` instead. It fails loudly when absent and never skips, but it is not the self-contained fixture the plan asks for. | test-architecture |
| `aex-wire` must emit the eighteen generated fragment server traits and their mount functions. `aex_wire::server` currently has only common request/response shapes, so `aex-central-http` cannot bind the generated central route set without inventing a second route table. | contracts |
| `aex-wire` needs six error codes the central plane returns and the registry lacks: `workspace_provision_pending`, `idempotency_in_flight`, `commit_outcome_unknown`, `last_owner_required`, `resource_conflict` and `invalid_scope`. `resource_deleted` maps onto the existing `gone`. Until they exist the HTTP layer cannot render those failures with their own code. | contracts |
| `aex-wire` must expose the canonical route-template string on the generated server trait. My replay identity binds it, and today it comes from `RouteDescriptor::template`, which works but is not the trait-level fact the plan names. | contracts |
| The generated route table does **not** admit a workspace key on `workspaces_list`, `workspace_get`, `central_operations_list`, `central_operation_get` or `central_operation_cancel`, though plan §5.3 marks all five `K ✔ own`. I followed the generated table, because admitting access the contract does not advertise is worse than a missing capability. Confirm which is intended. | contracts |
| `organizations_list`, `organization_create` and `workspaces_list` are `pause_exempt = false` in the generated table but resolve **no organization**, so the `402` gate would have nothing to evaluate. `requirement()` computes them exempt by construction and a test asserts the pairing over all 27 central routes. Setting `pause_exempt = true` on those three rows would make the two tables agree literally. | contracts |
| `aex-internal-contracts::assertion::AuthorizationAssertion` is a JSON claim set. The assertion on the wire is the 323-byte binary envelope this stream publishes (D-01). The internal contract should carry the envelope as a base64url string rather than re-describing its claims, or the two will drift. | contracts |
| The regional stream must accept `SignedEpochFrame` and `SigningKeyPublication` on its internal control endpoint and apply only **monotone** epoch advances. | regional services |
| The finance stream must supply `finance.account_state_v1(organization_id, status, reason, revision, changed_at)` with `GRANT SELECT TO aex_authz`, and `finance.ensure_account(uuid)` `SECURITY DEFINER` with `GRANT EXECUTE TO aex_control_api`. My migration suite carries a view-shaped stub for the join; production absence is `503 account_state_unavailable`, which is correct anyway. | finance |
| The finance migrations must sort after `migrations/central/20260801000300_control_functions.sql` and must not renumber the four DDL files before it. | finance |

I touched **two files outside my declared ownership**: the root `Cargo.toml`, to
add `subtle` and `ed25519-dalek` to `[workspace.dependencies]` (both required by
the pinned assertion and verifier designs), and `release/units.toml`, to fill the
four `[unit.lambda]` blocks its own header comment invites each owning stream to
fill.

## 5. Decisions taken beyond the orchestrator conventions

| # | Decision | Rationale |
| --- | --- | --- |
| D-27 | The scope registry is **`aex_wire::ScopeId`**; no separate `authorization-scopes.v1.json` is created under `api/schemas/`. `ScopeSet` is a bitset whose bit `n` is `ScopeId::ALL[n]`, asserted against the discriminants. | Plan §1 assigned me a scope registry file; the contracts stream had already landed the same 28 scopes in the same order and generated them. A second file would be the third scope vocabulary — precisely the defect this stream exists to remove. |
| D-28 | `requirement()` derives `scope`, `pause_exempt` and the admitted principal kinds from the **generated** `RouteDescriptor`; only the role floor and the resource class live in my table. | The wire contract is the executable authority for what a route requires. Re-declaring its fields would create two tables to keep in step, which is the drift the generated table exists to prevent. |
| D-29 | A route that resolves no organization is pause-exempt **by construction**, whatever the descriptor says. | The `402` gate reads an organization's account state. On an org-less route there is nothing to read, so the gate is a no-op and calling it "enforced" would be false. A test asserts `!pause_exempt ⟹ resolves an organization` over all 27 central routes. |
| D-30 | The wire contract's `account` and `user_session` principal kinds both map onto one `PrincipalKindTag::AccountActor`; the credential is recorded as `ActorCredential::{DashboardSession, AccountToken}`. | Same person, same scopes, same role, different credential. Distinguishing them at admission would be the second credential path the brief forbids; distinguishing them in audit and revocation is where the difference actually matters. |
| D-31 | The assertion's `PrincipalKind` has a third arm, `UserSession = 3`, and the envelope layout is unchanged. | `resolve_session_for_workspace` issues *the same* assertion. A separate envelope would be a second credential format for the edge to verify. |
| D-32 | Consumption and revocation are **facts**, not timestamp comparisons: a consumed challenge and a revoked session report as such at every instant, including one before the recorded timestamp. Revoking a lapsed session or token is a typed refusal. | Found by a generated property: with a `<= now` comparison, a clock that moves backwards resurrects a single-use credential. A recorded fact cannot be un-recorded by a clock. |
| D-33 | `CommitFailure::RolledBack` is produced for a `23xxx`/`40001`/`40P01` raised **at commit**, not only for a reported rollback status. | A deferred constraint trigger raises at `COMMIT`, and PostgreSQL has then definitively rolled back. Calling that ambiguous would send a caller to reconcile a transaction that provably left no trace. Everything else — timeout, reset, throttle, unknown transaction id — stays `Unknown`. |
| D-34 | A lost or expired Data API **transaction** maps to `StoreError::Unknown`, not `Unavailable`. | `Unavailable` means nothing was applied. A transaction the service no longer knows about may have committed, and the two answers lead to opposite recoveries. |
| D-35 | An invitation may never offer the `owner` role, enforced by a domain check and an `inv_role_ck` that admits only `admin` and `member`. | Ownership is transferred deliberately. Handing it out by email makes the last-owner invariant depend on a mailbox. |
| D-36 | The audit `detail` document is built from an eleven-key closed set, and `detail_is_permitted` refuses anything else. | A detail assembled from a request body eventually carries a secret, and an append-only table is the worst place to discover that. |
| D-37 | The migration suite takes a lane-supplied PostgreSQL through `required_env!` rather than starting a container. | Forced by the `data-image-literal` rule; see §4. The suite fails loudly on an absent prerequisite and never skips, so the no-skip policy holds either way. |
| D-38 | Central actor memberships are projected as bounded JSON tuples, not a PostgreSQL composite array and not a second query. | `aex-rds-data` already has strict JSON decoding, while a composite-array decoder would enlarge the transport vocabulary for one schema. One statement preserves the pinned authorization I/O budget; requesting 1,001 rows lets the decoder refuse an actor above the public 1,000-row bound instead of truncating authority. |
| D-39 | Every time-bounded `AuthorizationReader` credential method receives the request's `OffsetDateTime`. | The old skeleton substituted `UNIX_EPOCH`, making every modern credential appear unexpired. Reading a process or database clock inside the adapter would also split one request across instants. The request edge owns the instant and passes it through. |
| D-40 | The Aurora control adapter refuses opaque continuation cursors until the port carries decoded, authenticated keyset claims. | Ignoring a cursor repeats the first page; decoding it without the cursor secret accepts attacker-controlled ordering state. A typed refusal is the only honest current behavior. |

## 6. Gate output

```
$ cargo fmt --all -- --check
(no output)

$ cargo clippy -p aex-identity-domain -p aex-identity-app -p aex-identity-aurora \
    -p aex-control-domain -p aex-control-app -p aex-control-aurora -p aex-rds-data \
    -p aex-central-http -p central-identity-api -p central-authz \
    -p central-control-api -p central-control-worker --all-targets -- -D warnings
(no output)

$ cargo clippy -p aex-control-aurora --all-targets --features integration-engines -- -D warnings
(no output)

$ cargo nextest run <the twelve owned packages> --all-targets
Summary [88.537s] 368 tests run: 368 passed, 0 skipped

$ cargo check --workspace --all-targets
Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 19s

$ cargo run -p aex-workspace-check -- registry build
aex-workspace-check: wrote release/test-registry.json and release/unearned-evidence.json
(the generated registries were unchanged)

$ cargo run -p aex-workspace-check
aex-workspace-check: 133 member(s) and 139 package(s) satisfy every structural and registry rule
aex-workspace-check: 542 unearned-evidence row(s) recorded in the source-rewrite phase

$ git diff --check
(no errors)
```

The PostgreSQL migration suite is compile- and lint-covered by
`integration-engines`, but was not executed locally because
`AEX_CENTRAL_PG_URL` was absent. Its lane prerequisite remains explicit and the
suite does not skip when invoked.

## 7. The assertion format as landed

Big-endian, constant **323 bytes**, signed Ed25519 over bytes `0..259`.

```text
off  sz  field                          body off  sz  field
  0   4  magic = "AEXA"                        0   8  issued_at_ms
  4   1  version = 0x01                        8   8  expires_at_ms
  5   1  alg = 0x01 (Ed25519)                 16   1  audience_plane   1=dev 2=prd
  6  16  kid (raw UUID)                       17   1  audience_region  1..5
 22   2  body_len = 235                       18   1  audience_service 1..5
 24 235  body                                 19   1  principal_kind   1=workspace_key
259  64  signature over bytes[0..259]                                  2=account_actor
                                                                       3=user_session
                                              20  16  principal_id
                                              36  32  credential_binding
                                              68  16  organization_id
                                              84  16  workspace_id
                                             100   1  workspace_region (== audience_region)
                                             101   1  account_state    1=active
                                                                       2=paused_top_up_required
                                             102   8  scopes (u64 bitset, registry order)
                                             110 125  epoch_subjects: 5 x { kind u8, id 16B,
                                                                            epoch u64 }
```

`audience_region` and `workspace_region`: `1=us-east-1 2=us-east-2 3=us-west-2
4=ap-northeast-1 5=eu-west-1`.
`audience_service`: `1=session-api 2=secret-api 3=observation-api 4=otlp 5=stream`.
`epoch kind`: `0=empty 1=user 2=membership 3=workspace 4=key 5=account`.

`credential_binding = SHA-256("aex/authz/binding/v1" ‖ principal_kind ‖
principal_id ‖ SHA-256(complete token))`.

A decoder fails closed on: a length other than 323, wrong magic, unknown version
or algorithm, a declared body length other than 235, an unknown `kid`, a bad
signature, an unknown discriminant in any fixed field, a scope bit outside the
registry, a used epoch slot after an empty one, an empty slot carrying a non-nil
id or non-zero epoch, a duplicate `(kind, id)`, a lifetime over 30 000 ms even
when validly signed, an audience or region mismatch, a credential-binding
mismatch, and a projected epoch ahead of the claimed one.

The golden for a fixed claim set is
`crates/aex-identity-domain/tests/assertion.rs::SIGNED_GOLDEN`, 518 hex
characters.

## 8. The scope vocabulary as landed

Twenty-eight scopes, one registry, `aex_wire::scopes::ScopeId`. Bit `n` of a
`ScopeSet` is `ScopeId::ALL[n]`.

| Bit | Scope | Bit | Scope |
| ---: | --- | ---: | --- |
| 0 | `account:read` | 14 | `workspace:read` |
| 1 | `organizations:read` | 15 | `sessions:read` |
| 2 | `organizations:write` | 16 | `sessions:write` |
| 3 | `memberships:read` | 17 | `sessions:delete` |
| 4 | `memberships:write` | 18 | `files:read` |
| 5 | `workspaces:read` | 19 | `files:write` |
| 6 | `workspaces:write` | 20 | `files:live` |
| 7 | `workspaces:delete` | 21 | `resources:read` |
| 8 | `api_keys:read` | 22 | `resources:write` |
| 9 | `api_keys:write` | 23 | `secrets:read` |
| 10 | `billing:read` | 24 | `secrets:write` |
| 11 | `billing:write` | 25 | `secrets:revoke` |
| 12 | `operations:read` | 26 | `telemetry:read` |
| 13 | `operations:write` | 27 | `telemetry:write` |

Derived sets, each asserted:

- `CENTRAL` = bits 0–13. `REGIONAL` = bits 14–27. They partition the registry.
- `OWNER` = `CENTRAL`. `ADMIN` = `OWNER` minus `workspaces:delete`.
  `MEMBER` = `account:read`, `organizations:read`, `memberships:read`,
  `workspaces:read`, `billing:read`, `operations:read`, `operations:write`.
- `WORKSPACE_KEY_MINTABLE` = `REGIONAL` plus `account:read`, `billing:read`,
  `operations:read`, `operations:write`, `workspaces:read`. A workspace key can
  never carry `organizations:*`, `memberships:*`, `api_keys:*`,
  `workspaces:write`, `workspaces:delete` or `billing:write`, whatever a request
  asks for — and asking for one is a refusal, not a silent narrowing.

Effective scopes are `token_scopes ∩ role_scopes(role)` for a person and
`key_scopes ∩ WORKSPACE_KEY_MINTABLE` for a key. The edge never re-derives them.

## 9. Second pass — the central HTTP composition and the four deployables

Branch `rw/central-2`, off `main` after the contracts stream emitted the 146
server-trait methods and after the store bodies landed. This closes the two
largest gaps in §2 and one that §2 recorded as blocked on a peer.

The store method bodies are **not** part of this pass: they were implemented and
merged in parallel, and this branch keeps them as landed.

### 9.1 `aex-central-http`

All **27** central routes are mounted, across the eight generated groups.

| Group | Routes | Served by |
| --- | ---: | --- |
| `ApiKeys` | 3 | `central-control-api` |
| `Bootstrap` | 1 | `central-control-api` |
| `CentralOperations` | 3 | `central-control-api` |
| `Organizations` | 5 | `central-control-api` |
| `Workspaces` | 4 | `central-control-api` |
| `Auth` | 2 | `central-identity-api` |
| `Billing` | 8 | `finance-api` |
| `Identity` | 1 | `finance-api` |

Every mount is a loop over `RouteGroup::routes()`; no template is written twice
and none is written by hand. `every_generated_central_route_is_mounted` drives
all 27 through `axum` and asserts each answers with an AEX envelope, and a path
outside the table answers an unrouted `404` with no envelope at all — so an
authored-but-unmounted route is a red suite rather than a runtime `404`.

`CentralServiceId::groups()` is the one owner map, and two tests hold it to a
partition: no route is served twice and the union is exactly the central route
table.

What the edge does, in the wire contract's precedence order:

- **The authorizer context is read with a closed key set.** An undeclared key is
  a refusal, because an authorizer that starts emitting a field the edge drops is
  how an authorization input stops being enforced without anybody noticing.
- **It is re-verified against its own window**, with the same thirty-second
  ceiling the assertion envelope uses. A context that claims longer is refused at
  parse; one outside its window is refused at every request. There is no grace
  period and no cached-positive extension.
- **The target is resolved before the decision, and the decision before the
  account-state read.** Reading the state of an organization the caller may not
  act in would be a side effect it is not entitled to cause, so `403` is decided
  before `402` — and a pause-exempt route never reads the state at all.
- **An unreadable account state rejects admission.** `503
  account_state_unavailable`, retryable, never a fallback in either direction.
- **Headers are strict in both directions.** A route that declares
  `Idempotency-Key` refuses a request without one; a route that declares none
  refuses one that supplies it; the same for `If-Match`.
- **The body bound is applied before the parser**, and the read itself is bounded
  at one byte over the limit, so an oversize body is refused after a bounded read
  rather than buffered whole.
- **Continuation cursors are decoded and minted here and nowhere else** — see
  §9.3.
- **A deployable cannot link a capability it did not declare.** `Grant<C>` has no
  constructor outside a `Declares<C>` implementation, and `admit()` refuses a
  configuration binding for an undeclared capability, an undeclared key, an
  absent binding and an off-plane ARN, all before a client is opened.

### 9.2 The four deployables

Each resolves a typed, total configuration inside its own declared
`config_env_namespace`, refuses any login role but its own, runs the capability
admission check before any client is opened, and serves `/internal/healthz` and
`/internal/readyz` with **fail-closed** readiness: a composition whose probes
have not answered serves `503`, and one that probed nothing is never ready.

| Deployable | Public routes | Capabilities | Notable |
| --- | ---: | --- | --- |
| `central-authz` | 0 | `authorization.read`, `assertion.sign` | `rds-data:ExecuteStatement` only — no transaction API at all. Readiness requires the **write probe to fail**. The signing key is unwrapped once per cold start, not signed per request. |
| `central-identity-api` | 2 | `identity.write` | Cannot link a control write or the signing capability. |
| `central-control-api` | 16 | `control.write`, `control.queue_publish`, `regional.control_invoke` | The regional endpoint map must cover every launch region, checked at start-up rather than at the first `POST /api/workspaces`. |
| `central-control-worker` | 0 | + `control.queue_consume`, `mail.send`, `signing_key.administer` | `Handler::for_topic` is total over `Topic`, so an outbox topic without a duty is a compile error. A partial batch names only uncommitted items. |

`issue_for_key` and `issue_for_actor` in `central-authz` are the two assertion
issues, both pure and both tested: effective scopes are computed there and never
re-derived at the edge, a revoked or lapsed credential never issues, and an
`Unavailable` account state is a refusal rather than an `Active` envelope.

The Lambda memory, timeout and reserved concurrency for all four are declared in
`release/units.toml`, not in `[package.metadata.aex]` — that key set is closed at
thirteen keys with `deny_unknown_fields`, so a `lambda` block there would fail
`aex-metadata-unknown-key`. Each deployable's `resource_envelope.rs` asserts its
unit row carries all three values.

### 9.3 The continuation-cursor gap, closed

§2 recorded that `AuroraControlStore` refused every non-empty cursor. The cause
was a real defect one level down: `decode_cursor` compared the whole claim set
including `last`, which is the position the cursor exists to *transport* — so
the only caller who could use it was one who already knew the answer. It now
compares the five fields a query fixes (endpoint, principal, scope, region,
filter digest) and returns the carried position; a test asserts a continuation is
readable without knowing it.

With that fixed, the port carries decoded, authenticated keyset state:
`PageRequest.after` is a `(created_at_ms, id)` and `Page.next` is the position to
continue from, `None` when the page did not fill. The five list statements gained
a null-tolerant `(created_at, id) > (…)` predicate, and `aex-central-http` is the
only place that signs or verifies a cursor. **D-40 is withdrawn.**

### 9.4 The migration suite

Migrated from `AEX_CENTRAL_PG_URL`/`required_env!` to
`aex_test_harness::containers::PostgresContainer::start()`, behind
`required-features = ["integration-engines"]`, which now turns on the harness's
`containers` feature. One digest-pinned engine per process, one fresh database
per case. The suite writes no image name, tag or digest of its own. A Docker
daemon that refuses the container fails the suite; it never skips. **D-37 is
withdrawn**, and so is the first `TODO(cross-stream)` in §4.

The include paths also moved: the finance stream renamed the bundle to
timestamped filenames, so the four owned files are now
`20260801000000_bootstrap`, `…000100_identity`, `…000200_control` and
`…000300_control_functions`.

### 9.5 The live companions

Four companions, eight cases each, in the house style: every case `panic!`s with
what it would have proved and why it cannot yet (`OD-07`). None self-skips, none
is `#[ignore]`d, and none is an empty target. They are the specification of what
the first credentialed run must demonstrate — including the two cases that decide
whether a control is real at all:
`the_read_only_role_is_denied_every_write_it_could_attempt` and
`the_control_role_cannot_reach_a_finance_object`.

### 9.6 What is still not done

| Gap | Why |
| --- | --- |
| The Aurora-backed `AuthApi` and `ControlApi` implementations. `run()` is generic over them and every other part of both compositions is exercised; `main()` reports the missing service and exits non-zero. | A placeholder would answer `201` for a workspace nobody provisioned. Refusing is the honest behaviour, and the composition around it is complete and tested. |
| `finance-api` mounts `Billing` and `Identity`. `aex-central-http` publishes `mount_billing_api` and `mount_identity_api`; the finance stream composes them. | Those two groups are finance handlers, and building them here would be a second owner for the money path. |
| No live receipt of any kind. | `OD-07`. |

### 9.7 Decisions taken in this pass

| # | Decision | Rationale |
| --- | --- | --- |
| D-41 | `CentralServiceId::groups()` is the single owner map from a central route group to the binary that serves it, asserted to be a partition of the central route table. | The alternative is each deployable listing its own routes, which is the second route table the generated projection exists to prevent. It also makes "who serves `/api/billing/balance`" a compile-time fact rather than a deployment question. |
| D-42 | The central plane's credential is the **authorizer context**, verified here, rather than the 323-byte envelope. | The envelope is what `central-authz` issues *to regional edges*. A central request never carries a raw credential to this crate: API Gateway invokes `central-authz` and hands back a resolved context. Verifying an envelope here as well would be a second central credential path. |
| D-43 | The context carries its own issue and expiry instants and the edge re-checks both against the thirty-second ceiling. | The same rule as `assertion::verify` re-checking a validly signed lifetime: an authorizer bug must not be able to lengthen the window. |
| D-44 | Precedence stages 1–9 render their own failures directly; only handler answers pass `dispatch::declared`. | Every route participates in the precedence table by definition, and no central route declares `account_paused` or `authentication_unavailable`. Filtering an edge-stage code against `RouteDescriptor::errors` would replace a correct `402` with a `500`. |
| D-45 | A route whose `alt_principal` is `anonymous` admits a request with **no** authorizer context, and its replay identity is one shared sentinel principal. | The two device-flow routes are unauthenticated by contract. The sentinel means two anonymous callers presenting the same `Idempotency-Key` for the same intent share one grant, which is why the accepted design puts a per-IP usage-plan throttle in front of them. |
| D-46 | `decode_cursor` compares the five *binding* fields and returns the carried position rather than comparing it. | See §9.3. The previous behaviour was unusable for its only purpose, and a failing test proves it. |
| D-47 | `Page<T>` returns a keyset **position**, not an opaque cursor. | The port holds no signing secret. A store that minted its own token would be a second cursor authority. |
| D-48 | `central-authz` declares `rds-data:ExecuteStatement` and no transaction call, and its readiness requires a write to fail. | A role that cannot open a transaction cannot write even if a statement tried to, and a read-only role that turns out to be able to write must be discovered before the first request rather than during one. |
| D-49 | Every deployable's `Probes` is one boolean per probe rather than a single `ready` flag. | `/internal/readyz` names the dependency that did not resolve. A single flag answers "not ready" and leaves an operator to guess which of four things is wrong. |

### 9.8 What a peer still owes

Everything in §4 that is not withdrawn above, plus:

| `TODO(cross-stream)` | Owner |
| --- | --- |
| The six error codes from §4 are still absent, so `workspace_provision_pending`, `idempotency_in_flight`, `commit_outcome_unknown`, `last_owner_required`, `resource_conflict` and `invalid_scope` are folded onto declared codes. | contracts |
| No central route declares `account_paused`, `authentication_unavailable` or `account_state_unavailable` except `account_get` and `dashboard_bootstrap_get`, yet every route can produce all three at precedence stages 2–6. | contracts |
| `finance-api` must mount `RouteGroup::Billing` and `RouteGroup::Identity` through `aex_central_http::{mount_billing_api, mount_identity_api}`. Nine of the 27 central routes are unserved until it does. | central finance |
| The two public device-flow routes need the per-IP API Gateway usage-plan throttle; the shared anonymous replay principal assumes it. | infrastructure |
| `cargo fmt --all` now exceeds the Windows command-line limit at 133 members (`os error 206`). Per-package `cargo fmt -p` works, and Linux CI is unaffected, but the gate as written no longer runs on this host. | delivery |

### 9.9 Gate output

```
$ cargo fmt --all
The filename or extension is too long. (os error 206)
  # 133 workspace members exceed the Windows command-line limit. Run per
  # package instead; done for every package this branch touches, after which
  # `git diff` shows no formatting-only change.

$ cargo clippy -p aex-identity-app -p aex-identity-aurora -p aex-control-app \
    -p aex-control-aurora -p aex-central-http -p central-identity-api \
    -p central-authz -p central-control-api -p central-control-worker \
    --all-targets -- -D warnings
(no output)

$ cargo clippy -p aex-control-aurora --all-targets --features integration-engines -- -D warnings
(no output)

$ cargo nextest run -p aex-identity-app -p aex-identity-aurora -p aex-control-app \
    -p aex-control-aurora -p aex-central-http -p central-identity-api \
    -p central-authz -p central-control-api -p central-control-worker
Summary [166.317s] 229 tests run: 229 passed, 0 skipped

$ cargo check --workspace --all-targets
(no output, exit 0)

$ cargo run -p aex-workspace-check
aex-workspace-check: 133 member(s) and 140 package(s) satisfy every structural and registry rule
aex-workspace-check: 513 unearned-evidence row(s) recorded in the source-rewrite phase
```

## 10. Third pass — one assertion, and `central-authz` serving it

Branch `rw/assertion`, off `main`. This closes the fracture both the central and
the regional handoffs recorded from opposite sides: §4 asked contracts to stop
re-describing the envelope's claims, and `regional-services.md` §Seams recorded
that "two assertion vocabularies exist and neither is served. One has to go."

### 10.1 The binary envelope wins; the JSON claim set is deleted

Three spellings of one exchange existed. There is now one.

| Removed | Where it lived |
| --- | --- |
| `AuthorizationAssertion`, `signing_input`, `credential_bound_signing_input`, `AssertionSignature`, `MAX_SIGNATURE_BYTES`, `MAX_KEY_ID_BYTES`, `SignedAssertionEnvelope`, `ResolvedSessionAssertion` | `aex-internal-contracts::assertion` |
| the local `verify`, `SignedAssertion`, the `KeyVerifier` port, `Ed25519Anchors` | `aex-regional-http::{assertion,authz}` |
| `Ed25519Anchors`, `AssertionRequest`, `AssertionResponse`, `HttpAssertionSource`, `RequestAuthority`, `Authorized` | `regional-observation-api::edge`, `regional-otlp::edge` (both files deleted) |
| `RegionalService` | `aex-identity-domain::assertion` |

The choice is not "the one with more code behind it". Four properties decided it,
and each is a defect the JSON form had rather than a feature the binary form
adds:

- **The binding cannot be a separate field.** `AuthorizationAssertion` carried no
  credential binding, so `SignedAssertionEnvelope` bolted one on beside the claim
  set and `credential_bound_signing_input` was invented to cover both. A verifier
  then had two things to get right and the covered bytes had two definitions. The
  envelope carries the binding at body offset 36, inside the signature.
- **The binding is stronger.** The JSON form compared a bare `SHA-256(token)`.
  The envelope binds `SHA-256("aex/authz/binding/v1" ‖ principal_kind ‖
  principal_id ‖ SHA-256(token))`, so an envelope minted for a browser session
  cannot admit a workspace-key request that presents the same token digest.
  `a_browser_session_issues_the_same_envelope_bound_to_its_own_credential`
  asserts the two bindings differ and that each refuses the other.
- **Revocation is attributable.** The JSON form carried three unattributed
  counters — `keyEpoch`, `accountEpoch`, `revocationEpoch` — with no subject ids,
  so a membership or user revocation could not be expressed at all and a
  browser-session assertion was structurally unrevocable. The envelope carries
  five `(kind, id, epoch)` slots and the edge checks each against the subject it
  belongs to.
- **Nothing signed the JSON form.** `central-authz::issue_for_key` and
  `issue_for_actor` already produced the envelope. Choosing JSON meant writing a
  signer and deleting 859 source and 689 test lines with a hex golden and a
  323-byte single-bit mutation sweep; choosing the envelope meant deleting ~200
  lines of verification and delegating.

What the exchange keeps is SD-01: the request names the key by
`(keyId, presentedDigest)` and never carries it. `ResolveSessionForWorkspace`
**gains** `presentedDigest`, which closes the gap §4 recorded as
"`ResolvedSessionAssertion` carries a claim set and nothing that authenticates
it": a session assertion is now credential-bound and therefore verifiable rather
than replayable by anyone who obtains it.

`aex-internal-contracts::assertion` now owns the exchange and not the artifact:
`AssertionAudience`, `CredentialDigest`, the two requests, `IssuedAssertion` (the
envelope in canonical base64url) and `AssertionResponse`. `AssertionAudience` is
also the envelope's own `service` field — `RegionalService` was a second closed
five-member enum for the same fact — and `aex-identity-domain` keeps only the
byte codec, with `the_audience_codec_is_total_and_is_declaration_order` pinning
the two together. The layout is unchanged and `SIGNED_GOLDEN` still holds.

### 10.2 `central-authz` serves it

`run()` mounted `aex_central_http::health::router` under `lambda_http` and
nothing else, so both issue paths were unreachable and no assertion was ever
issued. It now resolves its start-up material, gates on every probe, and enters
`lambda_runtime::run`.

**Routes mounted: 0, and that is the served number.** `CentralServiceId::groups()`
assigns this binary none of the 27 central routes (D-41), and it is invoked
rather than routed — there is no HTTP integration, so an API Gateway proxy event
never arrives. The health router is therefore **removed rather than moved**: a
path no invocation can carry is a route that cannot be served, which RS-18
forbids. `release/units.toml` drops `health_path` and `ready_path` for the same
reason, one layer out. The served surface is **two invoke operations**:

| Operation | Path | Answers |
| --- | --- | --- |
| `ResolveWorkspaceKey` | `resolve_workspace_key` → verify digest → `issue_for_key` | `AssertionResponse` |
| `ResolveSessionForWorkspace` | `resolve_session_for_workspace` → verify digest → `issue_for_actor(UserSession)` | `AssertionResponse` |

Readiness became a **start-up gate**. With no endpoint to report `503` on, "not
ready" and "not running" are the same state and the honest one is the second;
`unresolved()` names the first probe that did not answer.

Three decisions carry weight:

- **A refusal is an answer; a fault is an error.** `AssertionRefusal` has exactly
  two arms, `not_authorized` and `account_state_unavailable`, because those are
  the two outcomes a caller renders differently. Unknown key, wrong digest,
  revoked, lapsed, wrong region and inactive workspace all collapse into the
  first, so an unauthenticated caller cannot learn which by asking. An unreachable
  store, an unheld pepper version and a malformed envelope are invocation
  **errors**: a caller must never say "your key is invalid" when the truth is "we
  could not tell". `an_unverifiable_credential_is_a_fault_and_never_a_refusal`
  holds the line.
- **The signing key is taken from the authority and refused if it is not ours.**
  `active_signing_key()` names both the `kid` and the `secret_ref`; start-up
  refuses a `secret_ref` the deployable did not declare, because otherwise one
  database write could redirect this process at any secret its execution role can
  read. It then self-tests the derived public key against the published one, so a
  wrong secret is a start-up failure rather than a fleet-wide verification outage
  discovered by customers.
- **An unrecognised payload is refused by name.** The API Gateway REQUEST
  authorizer this binary's doc comment described has no landed request or
  response contract, and `aex-central-http` (D-42) consumes an authorizer context
  nothing produces. Rather than invent it, `Invocation::classify` refuses the
  event explicitly, so the gap fails loudly at the first authorizer invocation
  instead of being answered with a plausible assertion. **This is owed work, not
  a closed item** — see §10.5.

`AEX_CENTRAL_AUTHZ_PEPPER_SECRET_ID` is new: the pepper ring is what turns a
transmitted digest back into a verifiable credential, so it is bound to
`authorization.read` rather than to `assertion.sign`.

### 10.3 The regional edge

`aex-regional-http` verifies the envelope through
`aex_identity_domain::assertion::verify` and keeps no verification of its own.
The `KeyVerifier` generic disappears from `RegionalEdge` entirely — the key set
is the concrete input, not a port.

The floors are **two explicit stages**, and the split is forced rather than
stylistic. A credential names its own key and nothing else; the workspace is a
fact only the assertion carries, and an unverified claim must not choose which
projection row is read.

| Stage | Type | Unnamed subjects answer |
| --- | --- | --- |
| 1 — before the placement is known | `CredentialFloors` | `Epoch::NEVER`, **deferring** to stage 2 |
| 2 — every request, cache hit or not | `RegionalFloors` | `u64::MAX`, so the assertion is stale and refused |

Stage 2's fail-closed answer is the load-bearing one: the
`regional-authz-projection` holds nothing about a person or a membership, so an
envelope carrying either cannot be checked here and is refused rather than
admitted for want of a reason to refuse it.
`an_assertion_carrying_a_subject_this_region_cannot_project_is_refused` and
`an_assertion_for_a_person_is_refused_by_a_regional_edge` assert both halves.

`PresentedCredential` parses a workspace API key at construction — it is the only
credential a regional host accepts — which removes the three sites that each
re-parsed it and could have disagreed.

### 10.4 The two private copies are gone

`regional-observation-api/src/edge.rs` and `regional-otlp/src/edge.rs` were
byte-identical apart from one constant. Both files are **deleted**. Both
deployables now compose `aex_regional_http::authz` and admit through
`RegionalEdge`, so the platform has one edge rather than three.

This was not only deduplication. The private edge passed
`ProjectedEpochs::default()` and read no projection at all, so on both
deployables a revoked key kept working for the full assertion lifetime and a
paused account was never gated. Both now read the revocation floor and the
placement on every request, enforce the pause gate and the declared replay
identity, and refuse a workspace placed in another region.

The behaviour the brief named as the one to remove — `HttpAssertionSource`
sending `{ "credential": "<the customer's key>" }` to
`/internal/authz/assertions`, a path `central-authz` does not expose — is gone
with the file. `the_issue_path_never_names_a_plaintext_credential` and
`neither_request_can_carry_a_credential` assert from both ends that what crosses
the region boundary is a digest.

Configuration follows the transport: `AEX_CENTRAL_AUTHZ_URL` →
`AEX_AUTHZ_FUNCTION_ARN` (IAM-invoked, no endpoint),
`AEX_ASSERTION_TRUST_ANCHORS` → `AEX_AUTHZ_VERIFY_KEYS_PARAM` (a rotation must
not need a redeploy of every regional service), plus a new
`AEX_AUTHZ_PROJECTION_TABLE`. `reqwest` and `aws-lc-rs` leave both manifests.

The trust-anchor document changes shape, because the envelope's `kid` is a raw
UUID at a fixed header offset rather than a free string:

```json
{ "schemaVersion": 1,
  "keys": [ { "keyId": "0193f0a1-…", "publicKey": "<43 chars base64url>",
              "notAfterMs": 1798761600000 } ] }
```

`notAfterMs` is when the region stops accepting the key, which is what retires a
rotated-out anchor without a redeploy.

### 10.5 What a peer still owes

| `TODO(cross-stream)` | Owner |
| --- | --- |
| The API Gateway REQUEST authorizer. No authorizer request or response shape is in the contract tree, and `aex-central-http` (D-42) consumes a context nothing produces. `Invocation::classify` refuses the event by name today, so the central plane's public routes cannot authenticate until this lands. | contracts + central identity |
| Nothing writes the `regional-authz-projection`. `RegionalFloors` is fail-closed, so an absent or half-written table refuses every request rather than admitting one. The placement row must carry the workspace's organization, which `WorkspacePlacement` already declares. | central-control-worker |
| No regional edge presents a browser session: `PresentedCredential` accepts a workspace key only. `ResolveSessionForWorkspace` is served and credential-bound, but which surface presents an `aex_ds_` token to a regional host — and how — is decided by no accepted record. | clients + regional services |
| `AEX_CENTRAL_AUTHZ_PEPPER_SECRET_ID` and the signing secret are plain Secrets Manager values with no rotation ceremony. The pepper ring holds up to eight versions and refuses an unknown one as a fault, so a rotation is expressible; nothing performs one. | delivery |
| `aex-identity-domain` now depends on `aex-internal-contracts` (audience vocabulary) and `aex-regional-http` on `aex-identity-domain` and `aex-control-domain` (the envelope and the epoch vocabulary). All are pure and link no AWS SDK, which the identity module's own doc comment anticipated. Confirm the edge is acceptable. | orchestration |

### 10.6 Decisions taken in this pass

| # | Decision | Rationale |
| --- | --- | --- |
| D-50 | The 323-byte binary envelope is the one assertion wire form; the JSON claim set, both signing inputs, the detached-signature type and the signed envelope are deleted | See §10.1. The JSON form could not carry the credential binding or the account state, so the binding had to sit outside the signature and the covered bytes had two definitions. |
| D-51 | `AssertionAudience` is the one audience vocabulary and lives in the contract crate; `aex-identity-domain` owns only the byte codec | `RegionalService` was a second closed five-member enum for the same fact, and the requests already carried the first. A totality test pins declaration order to wire order. |
| D-52 | The invoke answer is `AssertionResponse`, with a refusal as a **successful** invocation and an internal failure as an invocation error | A caller renders the first to a customer and retries the second. Reporting a revoked key through `function_error` would have made it indistinguishable from a central outage and retried forever. |
| D-53 | `AssertionRefusal` has exactly two arms | Unknown key, wrong digest, revoked, lapsed, wrong region and inactive workspace are one outcome to a caller and must be one outcome on the wire, or the exchange is an oracle. |
| D-54 | `central-authz` mounts no router at all and readiness is a start-up refusal | It is invoked, not routed; no HTTP request can reach it. A mounted `/internal/healthz` would be a route it can never serve (RS-18), and `units.toml` drops the declaration for the same reason. |
| D-55 | Start-up refuses a `secret_ref` the deployable did not declare, and self-tests the signing key against the authority's published public half | Otherwise one database write redirects the process at any secret its role can read, and a wrong key is discovered as a fleet-wide verification outage rather than a start-up failure. |
| D-56 | `ResolveSessionForWorkspace` gains `presentedDigest` | A session assertion that named no credential could be replayed by anyone who obtained it and no verifier could tell. It is now bound the same way its sibling is, and the plaintext session token stays regional too. |
| D-57 | The regional floors split into `CredentialFloors` and `RegionalFloors`, and an unprojectable subject answers a floor no assertion can satisfy | The workspace is a fact only the assertion carries, so the placement cannot be read before the assertion verifies — but the epoch check needs it. Stage 2 runs on every request, so nothing is admitted on stage 1 alone, and a subject this plane holds nothing about is refused rather than defaulted to "no reason to refuse found". |
| D-58 | `PresentedCredential` parses a workspace API key at construction | It is the only credential a regional host accepts. Three sites re-parsed it; one parse cannot disagree with itself. |
| D-59 | `plane_name` resolves to the typed `Plane` rather than a validated `String` | The plane a process reports and the plane it will accept an assertion for must be one value. `Plane::as_str` returns the verbatim spelling for the encryption context. |
| D-60 | `regional-otlp`'s `EffectiveLimits.json_body_bytes` is the OTLP encoded ceiling | Every route it owns carries an OTLP body, so the edge's single body bound is that one. A deployable owning both classes would need the edge to take the class from the route table; this one does not, and a second knob nothing reads is a knob that will eventually be set wrong. |

### 10.7 Gate output

`AWS_LC_SYS_PREBUILT_NASM=1` and `CARGO_BUILD_JOBS=4` were set for every Cargo
gate. `cargo fmt --all` still cannot run on this host (`os error 206`, §9.8); the
equivalent gate was run as `cargo fmt -p <package>` over every touched package.

```
$ cargo clippy -p aex-internal-contracts -p aex-identity-domain -p aex-regional-http \
    -p central-authz -p regional-observation-api -p regional-otlp \
    -p regional-session-api -p regional-secret-api -p regional-stream \
    -p content-lifecycle-worker -p regional-secret-key-admin -p session-operation-worker \
    --all-targets -- -D warnings
    Checking aex-regional-http v0.1.0 (…\crates\aex-regional-http)
    Checking regional-otlp v0.1.0 (…\services\regional-otlp)
    Checking content-lifecycle-worker v0.1.0 (…\workers\content-lifecycle-worker)
    Checking regional-stream v0.1.0 (…\services\regional-stream)
    Checking aex-internal-contracts v0.1.0 (…\crates\aex-internal-contracts)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.14s

$ cargo nextest run <the twelve packages above>
    Summary [ 210.945s] 441 tests run: 441 passed, 0 skipped

$ cargo check --workspace --all-targets
    Checking aex-regional-http v0.1.0 (…\crates\aex-regional-http)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.37s

$ cargo run -p aex-workspace-check
aex-workspace-check: 133 member(s) and 140 package(s) satisfy every structural and registry rule
aex-workspace-check: 501 unearned-evidence row(s) recorded in the source-rewrite phase

$ cargo run -p aex-workspace-check -- registry build
aex-workspace-check: wrote release/test-registry.json and release/unearned-evidence.json

$ git diff --check
(no output)
```

No test is `#[ignore]`d, self-skipping or retried to green, and nothing in this
pass was deployed, credentialed or run against AWS.
