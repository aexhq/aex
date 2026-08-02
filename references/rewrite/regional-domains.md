---
title: Regional pure domain and application crates — as landed
description: What the regional-domains stream implemented, what it deferred, the exact types and port traits it publishes, what it needs from peers, and the decisions it took beyond plan 04.
keywords:
  - rust
  - session domain
  - durable operations
  - content addressing
  - merkle
  - secret custody
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-01
related:
  - references/rewrite/contracts.md
  - references/rewrite/test-architecture.md
---

# Regional pure domain and application crates — as landed

Plan of record: `references/rust-native-rewrite-2026-07-31/plans/04-regional-domains.md` in the parent workspace.

Six crates, all pure: no AWS SDK, no HTTP, no environment read, no wall clock, no
globals, no `async` in the five domain crates. `Clock` and `IdFactory` exist only
in `aex-session-app` (D-01).

## 1. Implemented

| Crate | Modules | State |
| --- | --- | --- |
| `aex-content-domain` | `digest, path, placement, descriptor, identity, tree, pin, gc, denial, missing` | complete |
| `aex-operation-domain` | `operation, admission, lease, due, cursor, deletion, redact` | complete |
| `aex-secret-domain` | `plaintext, context, secret, revocation, custody` | complete |
| `aex-workspace-domain` | `registry, upload, grant, persist` | complete |
| `aex-session-domain` | `ids, budget, session, message, run, agent, journal, approval, terminal, deletion, pause, lineage, idempotency, testing` | complete |
| `aex-session-app` | `plan, ports, error, use_cases, testing` | plan and ports landed; 8 of 14 declared use cases |

The interrupted stream's `wire_pending` and `canonical` stand-in modules are
**deleted**. `aex-wire` and `aex-internal-contracts` now supply those types, and
`aex_wire::canonical` is the one canonicalizer (D-23).

## 2. Deferred, as tracked gaps

1. **Six use cases.** Plan 04 declares 14 functions, not 13. Eight now exist:
   the seven originally landed functions plus `rebind_credentials`. The six
   unwritten functions and their exact independent blockers are:

   | Function | Exact blocker outside `aex-session-app`'s implementable assembly |
   | --- | --- |
   | `create_session` | No accepted application command or domain planner binds the generated create request's resolved configuration, initial registry root, first custody row, root-agent budget and idempotency receipt into one replayable projection. |
   | `persist_workspace` | `ContentRoot` does not retain a root-page identity from which `ContentReader::load_page` can materialize the durable `TreeView`; `Write` also has no tree-page or persist-receipt write. The public `rootHash: ContentHash` and the domain's BLAKE3 root bytes additionally lack a specified conversion. |
   | `clone_session` | A first admission can mint the target, but an exact operation replay cannot recover that target session from the specified `OperationResult`; no accepted internal result/projection seam records it. Inventing a target-id payload would be a new wire contract. |
   | `discard_workspace` | No domain planner or transaction `Hint` carries the runtime generation-termination intent/receipt. Clearing `Session::generation` alone would acknowledge discard before the runtime authority accepts termination. |
   | `respond_approval` | The reader ports expose neither the stored approval nor its current 11-field binding, and `Condition` has no pending-approval compare-and-set condition. `ItemPresent` cannot preserve first-decision-wins. |
   | `continue_operation` | No port reads the `WorkItem`, and the operation domain has no total transition consuming `StepOutcome`: cursor persistence, retryable-step state and `Running -> Cancelled` after a worker observes cancellation are unowned. |

   No `wire_pending` module or callable stub was added for these six gaps. The
   functions remain absent and unreachable until their named owner contracts
   land; this is the exact blocked marker, not a fallback implementation.
2. **Property rows 11–14** (idempotency) are satisfied against
   `aex_wire::canonical::intent_digest` rather than a local hash, because the
   contract crate now owns the canonicalizer and the cross-language corpus.
   Row 13's TypeScript fixture generator is the contracts stream's.
3. **Row 99's `trybuild` corpus** is replaced by a source scan plus a
   behavioural check (see D-30 below).
4. **P5 mutation testing** was not run as a separate pass. The guard-removal
   property it asks for is covered structurally: `guard_before_mutation` proves
   `validate_append` returns exactly what the fold would for every state, and the
   plan-condition table in `aex-session-app` names each command's required
   conditions explicitly.

### Credential rebind continuation

`rebind_credentials(&AppContext, &Rebind)` now assembles the landed custody
domain and plan vocabulary without a committer or external mutation:

- account gating precedes operation replay;
- an exact replay returns the stored operation with an empty, validated plan and
  performs no idle, secret or custody read, so it cannot advance custody twice;
- first admission requires command-idle plus runtime true-idle, conditions on
  session/deletion/custody revisions and every selected revocation epoch, writes
  the operation, custody and session head together, and emits the old owner-key
  edge only as `Hint::DestroyKeyEdge` after commit;
- the durable operation result is the canonical generated
  `CredentialRebindResult`, not an ad-hoc payload;
- a clone that started with `CloneCredentials::None` admits its first custody
  row through the existing `admit_custody` transition.

`AppError` now carries `CustodyRejection` and canonical-result errors, maps a
true-idle rejection to `session_not_idle`, and maps the landed
`ApprovalRejection::BindingChanged` arm to `approval_binding_changed`.

## 3. Exact types and port traits published

### To the adapter stream

```rust
use aex_session_app::plan::{
    SessionTransaction, TransactionIntent, Condition, Write, Hint,
    ItemKey, TableFamily, ConditionId, PlanShape, PlanError, Planned,
    MAX_ACTIONS,        // 100
    MAX_BYTES,          // 4 MiB
};
use aex_session_app::ports::{
    Clock, IdFactory, SessionReader, RegistryReader, ContentReader,
    SecretCustodyReader, LimitsReader, AccountStateReader, ReservationAuthority,
    ContinuityReader, LiveWorkspaceReader, AuthorityCommitter,
    AppContext, SessionSnapshot, AgentPage, PageBudget, PortError,
    CommitError, CommitOutcome, ReservationRequest, ReservationGrant,
    WorkspaceContinuity,
};
```

Contract, unchanged from plan 04 §9: each `Condition` maps 1:1 onto one provider
condition expression and may not be dropped, weakened, merged or reordered; each
`Write` carries its own `TableFamily` and the adapter routes by that alone;
`Hint` is emitted strictly after a successful commit and never influences it;
`plan.validate()` runs before submission and a `PlanError` is an internal fault,
not a customer error; `CommitError::ConditionFailed { failed }` returns the exact
`ConditionId`s, which are positional and stable within one plan.

`Write::target() -> ItemKey` and `Write::family() -> TableFamily` are the routing
surface; `Condition::family()` is its read-side twin.

### To Brain

```rust
use aex_session_domain::{
    JournalEntry, JournalPage, JournalSeq, EntryIdentity, JournalBody,
    AuthorityFact, fold_control, validate_append, JournalError,
    AgentControl, AgentFence, AgentStatus, PublicAgentStatus, AgentTerminal,
    OpenEffectSet, EffectId,
    Run, RunStatus, RunOutcome, InterruptReason, TerminalAttempt, claim_terminal,
    TerminalCommit, TerminalRejection, OutboxEvent,
    CancellationEpoch, Session, SessionStatus, WorkAdmission,
    Approval, ApprovalBinding, BindingField, binding_drift,
};
use aex_content_domain::{ContentDigest, ContentRoot, Placement, placement_for};
use aex_secret_domain::{SessionCustody, managed_use_allowed};
```

`AuthorityFact` is new and is the seam D-02 needs: it is the **closed** set of
authority facts one journal entry may assert — `None`, `EffectOpened`,
`EffectSettled`, `ApprovalRaised`, `ApprovalResolved`, `Terminal`. Brain writes
the payload and names the fact; the session authority folds the fact and never
looks at the payload. Brain must not define a second journal envelope, agent
status enum, approval binding or terminal barrier.

### To the deployable stream

`regional-session-api` uses the `aex-session-app` use cases and `Planned<T>`;
`session-operation-worker` uses `aex_operation_domain::{claim, renew, complete,
plan_due_scan, backoff}`; `content-lifecycle-worker` uses
`aex_content_domain::{sweep_decision, unwrap_allowed}`; `regional-secret-api`
uses `aex_secret_domain::{set, delete, revoke, SecretPlaintext}`;
`regional-stream` reads `aex_session_domain::OutboxEvent` only.

### To observation and usage

`UsageClosureId`, `MeasurementId` (from `aex-wire`), `ReservationId`,
`RunOutcome`, `OutboxEvent` and `ContentRoot::logical_bytes`.

## 4. Changes needed from peers

1. **Resolved:** `aex-wire` now publishes `approval_binding_changed`, and
   `AppError::code` maps `ApprovalRejection::BindingChanged` onto it.
2. **Resolved:** the generated route table now uses `clone` and
   `trash`/`restore`/`purge`; no `fork` or `delete` alias remains.
3. **`aex-wire::limits` has no `session.materialized_agents` row.** Read as
   `session.subagent_concurrency`, which is exactly the right semantics — see
   D-27. No new row is needed unless the root agent is meant to consume budget,
   which D-22 says it is not.
4. **Resolved:** `OutboxEvent` lives in `aex-internal-contracts` and
   `aex-session-domain` re-exports it.
5. **`aex-wire` mints no `GrantId`, `CursorId`, `WorkId`, `OwnerId`,
   `ReservationId`, `UsageClosureId` or `OwnerKeyEdgeId`.** All seven are internal
   identities and are declared locally (`aex-content-domain::identity`,
   `aex-operation-domain::lease`, `aex-session-domain::ids`,
   `aex-secret-domain::custody`). If any of them becomes public, it moves to
   `aex-wire` and these become re-exports.
6. **`aex-runtime-control` owns `TrueIdle`.** `aex-secret-domain::custody::TrueIdle`
   is a verdict type — an instant plus an optional `TrueIdleViolation` — that the
   runtime stream produces and this stream only consumes, so the two cannot each
   compute a different answer. The runtime stream must also either produce or
   delete `ColdContinuityReason::{not_started, idle_retention_elapsed}`.
7. **`aex-internal-contracts::journal::JournalEntryKind` is closed with 22 arms.**
   Confirmed landed and matched exhaustively. Do not add a catch-all.

## 5. Decisions taken beyond plan 04

| # | Decision | Rationale |
| --- | --- | --- |
| D-26 | `DeletionState`, `DeletionEpoch` and `DeletionGuard` live in `aex-operation-domain`; `aex-session-domain::deletion` re-exports them and owns the transitions | Operation admission has to fence against the guard, and `aex-operation-domain` is the lower crate. The alternative was a second copy of the state enum, which is exactly the drift this stream exists to prevent. |
| D-27 | The materialized-agent ceiling counts non-terminal **subagents** against `LimitId::SessionSubagentConcurrency`, and depth against `SessionSubagentDepth` | The landed limit registry has no `session.materialized_agents` row, and `session.subagent_concurrency` already means "how many subagents may be concurrently materialized". The root agent is not a subagent, so nothing is miscounted. |
| D-28 | `ContentDigest` is a re-export of `aex_wire::ids::ContentHash`, not a second newtype | A body digest crosses the customer boundary, so the wire owns its grammar. `PageDigest` stays local because a page digest has no public rendering contract. |
| D-29 | The Merkle boundary function is **level-salted**: level `n` reads bytes `[2n, 2n+2)` of one `blake3("aex.tree.split.v1" ‖ path)` digest | Reusing the leaf predicate at every level makes every branch child a boundary, so the tree never converges. The salt keeps every level content-defined and history-independent, which is the whole point of D-06. Depth is capped at 16 levels, which at a mean fanout of 64 exceeds any reachable workspace. |
| D-30 | Plaintext non-persistence (row 99) is enforced by a source scan plus a behavioural check rather than a `trybuild` corpus, and zeroization (row 101) by proving the chosen wrapper's contract rather than by `miri` | `trybuild` is not a pinned workspace dependency and adding a second compile driver to prove one negative is a poor trade; observing a buffer after its owner drops needs `unsafe`, which the workspace forbids. Both checks fail the build rather than warn. |
| D-31 | `backoff` derives its jitter fraction from the **work id alone**, not from `(id, attempt)` | A per-attempt fraction lets a later attempt land earlier than an earlier one once the cap is reached, which is the retry-storm shape backoff exists to prevent. `backoff_step` exposes the un-jittered schedule so the property is checkable. |
| D-32 | A journal duplicate is recognised by `(seq == tail && identity == last_entry)`, so `AgentControl` carries `last_entry: Option<EntryIdentity>` | Recognising a duplicate by position alone would silently accept a *different* entry claiming a folded position. The identity is a derived authority fact, so keeping it on the control record costs nothing. |
| D-33 | `aex-session-domain` and `aex-session-app` each expose a `pub mod testing` of pure, deterministic fixtures | The property suites are integration targets and cannot reach `#[cfg(test)]` helpers. `aex_wire::testing` sets the precedent. Nothing in either module is randomised or reads the environment. |
| D-34 | The registry `ETag` is the first 16 bytes of `blake3("aex.registry.etag.v1" ‖ kind ‖ revision ‖ digest)`, rendered as 32 lowercase hex characters | `aex_wire::types::ETag` is a bounded opaque string, not a `[u8; 16]`. Deriving it deterministically keeps the plan's guarantee that an adapter cannot invent one. |
| D-35 | Approval cancellation and expiry are separate terminal states | A stop, binding drift or continuity loss is not a clock expiry. Every raised approval now carries an explicit future deadline; a decision at or beyond it returns the terminal expiry commit the application must persist. |
| D-36 | `WorkspaceDelete` is the domain spelling and `ContentGc` is internal-only | The public contract already names workspace deletion. Keeping the same spelling removes an adapter alias; `OperationKind::public` is deliberately partial so internal maintenance cannot enter a customer response. |
| D-37 | Operation progress carries a bounded phase and public results decode under the envelope kind | The stored canonical result has no redundant discriminant. The envelope is authoritative, malformed bodies fail typed projection, and no generic JSON leaks through the generated ten-arm union. |

## 6. Reporting gate

```
cargo fmt --all
cargo clippy -p aex-session-domain -p aex-session-app -p aex-operation-domain \
  -p aex-workspace-domain -p aex-content-domain -p aex-secret-domain \
  --all-targets -- -D warnings
    (clean)
cargo nextest run -p aex-session-domain -p aex-session-app -p aex-operation-domain \
  -p aex-workspace-domain -p aex-content-domain -p aex-secret-domain
    Summary [ 299.384s] 261 tests run: 261 passed, 0 skipped
cargo check --workspace --all-targets
    (clean)
cargo run -p aex-workspace-check
    133 member(s) and 139 package(s) satisfy every structural and registry rule
```

Continuation verification for the credential-rebind increment (`9e44049d`),
with `CARGO_BUILD_JOBS=4`:

```
cargo fmt --all -- --check
    (clean)
cargo clippy -p aex-session-app --all-targets -- -D warnings
    (clean)
cargo nextest run -p aex-session-app
    Summary [ 37.937s] 25 tests run: 25 passed, 0 skipped
cargo check --workspace --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12m 30s
cargo run -p aex-workspace-check
    133 member(s) and 139 package(s) satisfy every structural and registry rule
    542 unearned-evidence row(s) recorded in the source-rewrite phase
```

No `#[ignore]`, no environment self-skip, no empty target, no retry-to-green.
`retries = 0` in every nextest profile. The five domain crates and the
application crate declare live testing structurally not applicable — pure crates
with no independent live seam — and no empty companion package was created.
