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
status: partial
last_verified: 2026-08-01
related:
  - references/rust-native-rewrite-2026-07-31/plans/02-central-identity-control.md
  - references/rust-native-rewrite-2026-07-31/plans/00-orchestrator-conventions.md
  - references/rewrite/contracts.md
  - references/rewrite/test-architecture.md
---

# Central identity, control and authorization — stream handoff

Branch `rw/central-identity`. Nothing is pushed. Three commits, each green at the
point it was made.

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

### The DDL, `migrations/central/0001`–`0004`

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

- The three pinned authorization statements, including
  `resolve_session_for_workspace`, which is byte-for-byte the account-token
  statement apart from the credential table. One credential path, not two.
- The pinned I/O budget is **counted**: resolving a key or a session is exactly
  one statement and zero transactions, asserted against a counting stub.
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
write probe really fails, and the three authorization statements `PREPARE`
successfully as that role.

## 2. What I deliberately left undone

Each is a tracked gap with a named blocker, not an oversight.

| Gap | Why, and what it costs |
| --- | --- |
| **The `IdentityStore` and `ControlStore` implementations.** The ports, the commands, the statements, the row decoders and the error mapping are all landed; what is missing is the ~30 methods that sequence them inside transactions. | The load-bearing decisions — statement text, transaction shape, guard placement, constraint mapping, the unknown-commit rule — are all landed and tested. What remains is mechanical binding, and doing it half-tested would have been worse than leaving it named. |
| **`aex-central-http`.** Still the skeleton from `main`. | It needs `aex-wire`'s generated server traits, which the contracts stream deliberately did not emit (their §2). Binding handlers against `ROUTES` by hand would be a second route table. |
| **The four deployables' bodies.** Their config parsing, startup denial and unit tests are the scaffolds from `main`; their Lambda shapes are now declared. | They compose crates that are not finished. Their `run()` still returns `NotImplemented`, which is honest. |
| **`resolve_account_token_central` / `resolve_dashboard_session_central`.** Typed `Fatal` with a pointer to this document. | The central-plane actor statement returns an `array_agg` of memberships in one row, and `aex-rds-data` decodes `text[]` but not a composite array. Choosing between a JSON projection and a second statement is a schema decision the dashboard-bootstrap route's shape settles, and that route's body is not authored. |
| **The four live companions.** Untouched. | `OD-07`: nothing is deployed or credentialed in this run, so no live receipt is earnable. |
| **`api/schemas/authorization-scopes.v1.json`.** Not created. | The contracts stream already landed the 28-scope registry at `api/schemas/registries/scopes.yaml`, generated into `aex_wire::ScopeId`. Creating a second scope file would be exactly the drift this stream exists to remove. Decision D-27 below. |

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
use aex_control_aurora::{AuroraAuthorizationReader, sql as control_sql,
    map_store_error, map_commit_failure};
use aex_identity_aurora::sql as identity_sql;
```

Also published for the finance stream: `control.bump_account_epoch(uuid)` — call
it in the **same transaction** as every pause and resume, or revocation is
bounded only by the thirty-second expiry.

## 4. What I need from a peer

| `TODO(cross-stream)` | Owner |
| --- | --- |
| `aex-test-harness` must expose a container builder (e.g. `containers::postgres() -> GenericImage`). The `data-image-literal` rule bans the container library's only constructor in every path under `/tests/` and exempts one directory — the harness — so **no product crate can start a container at all** today. My migration suite therefore takes a lane-supplied database through `required_env!("AEX_CENTRAL_PG_URL")` instead. It fails loudly when absent and never skips, but it is not the self-contained fixture the plan asks for. | test-architecture |
| `aex-wire` needs six error codes the central plane returns and the registry lacks: `workspace_provision_pending`, `idempotency_in_flight`, `commit_outcome_unknown`, `last_owner_required`, `resource_conflict` and `invalid_scope`. `resource_deleted` maps onto the existing `gone`. Until they exist the HTTP layer cannot render those failures with their own code. | contracts |
| `aex-wire` must expose the canonical route-template string on the generated server trait. My replay identity binds it, and today it comes from `RouteDescriptor::template`, which works but is not the trait-level fact the plan names. | contracts |
| The generated route table does **not** admit a workspace key on `workspaces_list`, `workspace_get`, `central_operations_list`, `central_operation_get` or `central_operation_cancel`, though plan §5.3 marks all five `K ✔ own`. I followed the generated table, because admitting access the contract does not advertise is worse than a missing capability. Confirm which is intended. | contracts |
| `organizations_list`, `organization_create` and `workspaces_list` are `pause_exempt = false` in the generated table but resolve **no organization**, so the `402` gate would have nothing to evaluate. `requirement()` computes them exempt by construction and a test asserts the pairing over all 27 central routes. Setting `pause_exempt = true` on those three rows would make the two tables agree literally. | contracts |
| `aex-internal-contracts::assertion::AuthorizationAssertion` is a JSON claim set. The assertion on the wire is the 323-byte binary envelope this stream publishes (D-01). The internal contract should carry the envelope as a base64url string rather than re-describing its claims, or the two will drift. | contracts |
| The regional stream must accept `SignedEpochFrame` and `SigningKeyPublication` on its internal control endpoint and apply only **monotone** epoch advances. | regional services |
| The finance stream must supply `finance.account_state_v1(organization_id, status, reason, revision, changed_at)` with `GRANT SELECT TO aex_authz`, and `finance.ensure_account(uuid)` `SECURITY DEFINER` with `GRANT EXECUTE TO aex_control_api`. My migration suite carries a view-shaped stub for the join; production absence is `503 account_state_unavailable`, which is correct anyway. | finance |
| `migrations/central/0005_finance.sql` and `0006_cross_schema_grants.sql` must not renumber `0001`–`0004`. | finance |

I touched **two files outside my declared ownership**: the root `Cargo.toml`, to
add `subtle` and `ed25519-dalek` to `[workspace.dependencies]` (both required by
the pinned assertion and verifier designs), and `release/units.toml`, to fill the
four `[unit.lambda]` blocks its own header comment invites each owning stream to
fill.

## 5. Decisions taken beyond the orchestrator conventions

| # | Decision | Rationale |
| --- | --- | --- |
| D-27 | The scope registry is **`aex_wire::ScopeId`**; `api/schemas/authorization-scopes.v1.json` is not created. `ScopeSet` is a bitset whose bit `n` is `ScopeId::ALL[n]`, asserted against the discriminants. | Plan §1 assigned me a scope registry file; the contracts stream had already landed the same 28 scopes in the same order and generated them. A second file would be the third scope vocabulary — precisely the defect this stream exists to remove. |
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

## 6. Gate output

```
$ cargo fmt --all
(no output)

$ cargo clippy -p aex-identity-domain -p aex-identity-app -p aex-identity-aurora \
    -p aex-control-domain -p aex-control-app -p aex-control-aurora -p aex-rds-data \
    -p aex-central-http -p central-identity-api -p central-authz \
    -p central-control-api -p central-control-worker --all-targets -- -D warnings
(no output)

$ cargo clippy -p aex-control-aurora --all-targets --features integration-engines -- -D warnings
(no output)

$ cargo run -p aex-workspace-check
aex-workspace-check: 133 member(s) and 139 package(s) satisfy every structural and registry rule
aex-workspace-check: 569 unearned-evidence row(s) recorded in the source-rewrite phase
```

`cargo nextest run` over the twelve owned packages and `cargo check --workspace
--all-targets` are recorded in the stream report.

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
