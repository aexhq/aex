---
title: Regional stores — landed state
description: What the regional-stores stream implemented, what it deliberately left as a tracked gap, the adapter APIs and table definitions it publishes, the changes it needs from peers, and the decisions it took beyond the orchestrator conventions.
keywords:
  - dynamodb
  - regional authority
  - adapters
  - handoff
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-01
related:
  - references/rust-native-rewrite-2026-07-31/plans/05-regional-stores.md
  - references/rust-native-rewrite-2026-07-31/plans/00-orchestrator-conventions.md
  - references/rewrite/test-architecture.md
  - references/rewrite/contracts.md
---

# Regional stores — landed state

Branch `rw/regional-stores`. Nothing is deployed, published or credentialed.

## 1. Implemented

### `migrations/regional/`

All eight table generation definitions, their JSON Schema, and the canonical
bundle Terraform consumes through `jsondecode`:

| File | Table |
| --- | --- |
| `tables/session-authority.json` | session heads, messages, runs, native events, approvals, agent control and journals, operations, receipts |
| `tables/regional-work.json` | durable runnable work over one sharded due index |
| `tables/regional-content.json` | body descriptors, inline ciphertext, Merkle pages, pins, grants, GC epoch and candidates |
| `tables/regional-registry.json` | current `(workspace, kind, name)` pointer, upload staging, receipts |
| `tables/regional-secret-custody.json` | secret metadata, hidden source generations, session custody, revocation epoch |
| `tables/regional-secret-keystore.json` | the provider-mandated hierarchical branch-key schema |
| `tables/runtime-activity.json` | Hands generation head, lifecycle intents and receipts, idle probes, due index |
| `tables/regional-authz-projection.json` | read-only workspace placement, key revocation, signed feed frontier |

`schema.json` enforces the properties that would otherwise be review
conventions: `PAY_PER_REQUEST`, deletion protection, 35-day PITR, an
`alias/aex-regional-*` CMK per authority, `INCLUDE`-only sparse indexes with an
exhaustive attribute list, `expiresAtEpochSeconds` as the only TTL attribute, a
closed `itemType` vocabulary, and a least-privilege IAM action list per role.
The loader in `aex-regional-test-support::tables` validates every file, proves
attribute closure both ways, and rebuilds
`migrations/regional/generated/regional-tables.json` deterministically; a stale
checked-in bundle is a test failure.

### `crates/aex-session-dynamodb`

The `session-authority` adapter, and with it the machinery every regional
DynamoDB adapter shares. Module map:

| Module | Owns | Feature |
| --- | --- | --- |
| `component` | key component validation, fixed-width sequence/shard/bucket renderings, `xxh3` shard selection | always |
| `attr` | attribute construction, the typed non-coercing row reader, the `NULL`-avoiding item builder | always |
| `measure` | item and page size preflight, the inline placement boundary | always |
| `plan` | `Participant`, `RegionalTables`, `TransactionPlan`, the one compiler | always |
| `error` | `StoreError`, cancellation decoding, the AWS-condition mapping table, `RetryPolicy` | always |
| `replay` | `IdempotencyScope`, `Receipt`, `ReceiptStore`, `Backoff`, `commit_or_replay` | always |
| `paging` | signed cursors, `PagePosition`, `PageBudget` | always |
| `keys`, `codec`, `transactions`, `store`, `wire_pending` | the `session-authority` table | `session-authority` |
| `projection` | the read-only `regional-authz-projection` reader | `authz-projection` |

Load-bearing properties, each asserted rather than documented:

- `TransactionPlan` **refuses** an action with no condition expression, and sets
  `ReturnValuesOnConditionCheckFailure = ALL_OLD` on every action itself, so
  neither can be forgotten at a call site.
- A cancellation's positional reason vector maps back to the participant that
  lost. A reason vector that does not match the plan is `Invalid`, never a guess.
- `CommitAmbiguous` is a distinct, non-retryable type carrying how to resolve it.
- `commit_or_replay` implements plan 05 §3.9 exactly: receipt first, one re-read
  on a receipt-participant loss or an ambiguous commit, a non-receipt
  precondition failure propagated unchanged, and bounded retries that re-enter at
  the receipt read so a retry can never produce a second commit.
- Cursors are HMAC-SHA-256 signed over RFC 8785 JCS bytes, bound to
  `(resource, organization, workspace)`, verified before the payload is parsed,
  and expire at 24 hours.
- §3.1 admission, §3.2 terminal barrier, §3.3 decision, §3.4 fanout page and
  §3.7 trash/restore/purge compile to exactly the declared participants in the
  declared order with the declared conditions.

121 tests, all passing.

### `crates/aex-work-dynamodb`

The `regional-work` authority: keys, closed vocabularies, typed payloads, row
codecs, claim/renew/fenced-commit/poison expressions, the sharded due scan and
per-shard reconciliation cursors. 45 tests, all passing.

It depends on `aex-session-dynamodb` with `default-features = false`, so the
session row codec and the projection reader are not in its link graph.

## 2. Deferred, with the reason

| Crate | State |
| --- | --- |
| `aex-content-dynamodb` | **not started.** §2.3 key templates, the SHA-256/BLAKE3 digest split, pins on roots, grant rows without a body copy, the GC epoch and the fenced sweep. |
| `aex-content-aws` | **not started.** §5 conditional create, multipart three-phase completion, presigned grants, `RedactedUrl`, fenced delete, `content_missing`. |
| `aex-registry-dynamodb` | **not started.** §2.4 pointer/ETag/upload state machine. |
| `aex-secret-custody-dynamodb` | **not started.** §2.5 plus the `REDACT#{session_id}` manifest and the `pcr_` directory. |
| `aex-secret-keystore-dynamodb` | **not started.** §2.6 typed `KeyStoreConfig` and read-only introspection. |
| `aex-secret-aws` | **not started.** See the `G-ESDK` outcome in §5 below: the arm is decided, the code is not written. |
| `aex-runtime-activity-dynamodb` | **not started.** §2.7. |
| `tests/live/aex-live-regional-stores/` | **not started.** §8.3 items 1–14. |
| `tests/load/regional-stores/` | **not started.** §8.4. |

Their table definitions **are** landed and asserted, so the schema half of each
is done and the adapter half is not.

Every remaining crate is still the compiling skeleton `main` shipped, so
`cargo check --workspace` stays green.

## 3. Published interfaces

- **`aex_session_dynamodb::plan`** — `Participant` (with the named constants for
  every participant plan 05 §3 pins), `RegionalTables`, `TransactionPlan`,
  `key`, `keyed`, `IMMUTABLE`. This is the one transaction compiler; no other
  crate may build a `TransactWriteItems` request.
- **`aex_session_dynamodb::attr`** — `Item`, `Row`, `ItemBuilder`, the attribute
  constructors, `CodecError`. Every regional codec is written on these.
- **`aex_session_dynamodb::component`** — key component validation and the
  fixed-width renderings. Any crate placing a value in a key uses it.
- **`aex_session_dynamodb::error`** — `StoreError`, `Resolution`, `Idempotence`,
  `RetryPolicy`, `classify`, `classify_code`, `decode_cancellation`.
- **`aex_session_dynamodb::replay`** — `commit_or_replay`, `ReceiptStore`,
  `Backoff`, `IdempotencyScope`, `Receipt`, `RECEIPT_RETENTION`,
  `OPERATION_RETENTION`.
- **`aex_session_dynamodb::paging`** — `CursorKey`, `CursorBinding`,
  `PagePosition`, `PageBudget`, `mint`, `verify`.
- **`aex_session_dynamodb::keys`** — the `session-authority` key templates,
  including `BRAIN_PREFIX` and `BRAIN_AGENT_PARTITION_PREFIX`.
  **`aex-brain-store-aws` calls this module rather than forking the item
  shapes.**
- **`aex_session_dynamodb::transactions`** — `ADMISSION_ORDER`,
  `TERMINAL_ORDER`, `DECISION_ORDER`, `Foreign`, `ForeignAction`, and the five
  `compile_*` functions.
- **`aex_work_dynamodb::claim`** — `enqueue`, `enqueue_dedupe`, `complete` and
  `poison` builders, so a peer can splice a work participant into its own
  transaction without knowing the row shape.
- **`migrations/regional/generated/regional-tables.json`** — the canonical bundle
  with a `blake3` content digest and the per-role IAM action lists. The
  infrastructure stream must not hand-write a table resource.

### The foreign-action seam

`regional-work`, `regional-content` and `regional-authz-projection` participate
in `session-authority` transactions, but those row shapes belong to their own
crates, and those crates depend on `aex-session-dynamodb` for the compiler.
Rather than invert the dependency or duplicate the rows, the session compiler
accepts pre-built actions from the caller and splices them at fixed positions:
`compile_admission(tables, plan, AdmissionForeign { placement, content, work })`.
`ADMISSION_ORDER` is published so a composing deployable can assert it supplied
what the order expects. The order itself is fixed here and a caller cannot
reorder a transaction by accident.

## 4. Changes needed from peers

| Requirement | Owner |
| --- | --- |
| **`aex-test-harness` owes a container-start helper.** `data-image-literal` bans `GenericImage::new(` outside `tests/support/aex-test-harness`, and the harness exposes `images::reference` but nothing that starts a container. As written, **no stream can write an engine-backed integration target at all.** Both landed crates carry `not_applicable.integration` naming this, and the DynamoDB Local cases from plan 05 §8.2 land the moment the helper exists. | test-architecture |
| The plan types in `aex_session_dynamodb::wire_pending` are peer-owned and every item names the path that replaces it: `AdmissionPlan`, `TerminalPlan`, `LifecyclePlan`, `AgentDecisionPlan`, `FanoutPagePlan`, `SessionHead`, `Run`, `Message`, `SessionEvent`, `AgentControl`, `JournalEntry`, `StoredOperation`, `WorkspacePlacement`, `KeyRevocation`, `FeedFrontier`. A different name costs a mechanical rename; the **participant-naming requirement is not negotiable**, because §7's decoding is exact only if a plan remembers what it put at each index. | regional domains, brain |
| `regional-authz-projection`'s item shapes are still assumed: `workspace_placement`, `key_revocation`, `feed_frontier` with the attribute names in `projection.rs`. `central-control-worker` is the only writer and must confirm or correct them. | central identity/control |
| The `usage.storage.delta`, `usage.compute.closure` and `usage.transfer.authorized` payload schemas in `aex_work_dynamodb::codec::payload_schema` are provisional field sets. The transactional delivery mechanism is settled; the names are not. | usage metering |
| `session-authority`'s `itemTypes` vocabulary does not yet include Brain's session-level item types. Brain items live in this table under `BRAIN#`, and the codec rejects an unlisted `itemType`, so Brain must add its types to `migrations/regional/tables/session-authority.json` when it lands them. The prefix is reserved and asserted; the vocabulary row is not. | brain |
| The content and secret CMK policies must add `StringEquals` on `kms:EncryptionContext:aex:workspace` and `aex:domain` per role (OD-18, plan 05 G-11). §6's context is designed for it and cannot enforce it alone. | infrastructure |
| The content bucket policy must deny `s3:PutObject`/`s3:CompleteMultipartUpload` with a null `s3:if-none-match`, deny `s3:DeleteObject` with a null `s3:if-match`, deny delete to any principal but the lifecycle role, and deny `s3:signatureAge` over 300000 ms (OD-17). | infrastructure |
| `aex-live-session-operation-worker` does not claim `aws.dynamodb.streams`, so `aex-work-dynamodb` no longer declares it (this crate writes rows; `regional-stream` and the pipes read the feed). If the stream contract needs a live claim, the companion is where it belongs. | delivery |

## 5. Decisions taken

| # | Decision | Rationale |
| --- | --- | --- |
| RS-01 | **`G-ESDK` fails. The envelope/AEAD arm of plan 05 §6.4 is the plan of record for `aex-secret-aws`.** | `aws-esdk` 1.2.4 depends on `aws-lc-sys ^0.39`, which cannot unify with the workspace's `aws-lc-rs 1.17.3` → `aws-lc-sys 0.43.0`. Adopting it would put **two copies of a security-critical native crypto library** in one link graph, and `aws-lc-sys 0.39.1` additionally fails to build on the pinned Windows build host (it requires NASM, which 0.43 does not). Neither arm of §6.4's confinement fallback helps: the problem is the dependency graph, not `Send`/`Sync`. The specified envelope arm keeps the identical key hierarchy, the identical encryption-context-as-AAD binding, the same `aws-lc-rs` primitives and the same bounded zeroizing cache; the only loss is the interoperable message format, which nothing outside Rust ever reads. The composition must select one implementation at startup and log which — never a runtime fallback. |
| RS-02 | The shared machinery lives in `aex-session-dynamodb` outside both features, and every other regional adapter depends on it with `default-features = false`. | Plan 05 B.1 puts the one compiler here, and D-21 requires that a projection-only consumer link no session write symbol. Putting the shared modules outside both features preserves the link-isolation property even for a binary that also pulls `aex-work-dynamodb`, which feature unification would otherwise defeat. |
| RS-03 | `commit_or_replay` takes `Fn`, not the plan's `FnOnce`. | Rule 5 of the plan's own contract says a retry re-enters at step 1, which calls the commit closure again. `FnOnce` cannot express that. |
| RS-04 | The combinator takes an injected `Backoff` port rather than sleeping itself. | An adapter that reads a clock and a random source cannot have its retry bound asserted without a timer. The policy stays data (`RetryPolicy::PINNED`); the waiting is the composition's. |
| RS-05 | A key component also rejects every control character and `U+FFFE`, not only `#`, `NUL` and `U+FFFF`. | The registry codec already rejects a control character in a name, and excluding the whole noncharacter neighbourhood keeps the range-scan sentinels unambiguous with no cost. |
| RS-06 | The compiler rejects an empty transaction plan. | A command that decided to do nothing should not have produced a plan; committing an empty transaction would report success for no effect. |
| RS-07 | An idempotency scope is a validated value with a closed base vocabulary and a checked arity, not a formatted string. | The scope enters a partition key. A free-text scope makes one route's replay key collide with another's. |
| RS-08 | `StoreError` derives `PartialEq` but not `Eq`. | `observed` carries raw `AttributeValue`s, whose numeric variant is decimal text with no total equality. |
| RS-09 | Timestamps are `aex_wire::Timestamp` throughout, never a second formatter. | Its `to_wire` is already the fixed-width 24-character spelling D-03 requires, and reusing it means the sort-key rendering and the wire rendering cannot diverge. |
| RS-10 | A session list entry is decoded into its own slim type, not into a whole `SessionHead`. | The index projection is deliberately slim; a list type with nowhere to put a prompt cannot start carrying one. |
| RS-11 | `aex-work-dynamodb` re-checks the payload schema on **decode** as well as encode. | A row written by an older revision, or by hand, must not become readable just because it is already stored — which is the only thing standing between `NEW_IMAGE` and a content leak. |
| RS-12 | A work priority band outside `PRIORITY_LEAD_SECONDS` is an error, never clamped to zero. | Silently promoting background work to the highest band is exactly the kind of "helpful" default that makes a scheduling incident unexplainable. |
| RS-13 | A lease renewal does not advance the fence. | Extending a lease is not a new claim; advancing the fence would invalidate the holder's own in-flight commit. |
| RS-14 | A poisoned work record gets no TTL. | It is forensic evidence referenced by an incident, not disposable state. Only a retired (`done`) record is reclaimed. |
| RS-15 | `migrations/regional/**` is pinned to LF in `.gitattributes`. | The bundle is compared byte-for-byte against a fresh generation; without the pin `core.autocrlf` makes that gate fail on a clean tree. |

## 6. Gate output

Run on the branch at the second commit, `CARGO_BUILD_JOBS=4`:

```
cargo fmt --all                                              clean
cargo clippy -p aex-session-dynamodb -p aex-work-dynamodb \
  --all-targets -- -D warnings                               clean
cargo nextest run -p aex-session-dynamodb                    121 tests run: 121 passed, 0 skipped
cargo nextest run -p aex-work-dynamodb                        45 tests run:  45 passed, 0 skipped
cargo check --workspace --all-targets                        clean
cargo run -p aex-workspace-check                             133 member(s) and 139 package(s) satisfy
                                                             every structural and registry rule
```

The seven crates not yet implemented have no tests to run, so they are absent
from the nextest lines rather than reported as zero-test passes.

No `#[ignore]`, no environment self-skip, no empty suite, no retry-to-green.
`.config/nextest.toml` pins `retries = 0` in every profile.

## 7. What could only be asserted locally

Everything above is proved against the serialized request, the compiled plan,
the row codec or the checked-in generation definition. None of it touches AWS.
The following remain unproved and belong to `tests/live/aex-live-regional-stores/`
(plan 05 §8.3), which is not written:

1. cross-table `TransactWriteItems` semantics, cancellation-reason ordering, and
   the `TransactionConflict` reason under real contention;
2. adaptive capacity, hot-partition throttling, and the `gsi_workspace_index`
   per-workspace ceiling;
3. TTL actually deleting an item — and, more importantly, the proof that no fence
   depends on it having fired;
4. PITR, restore into a new table, and the deletion-denial replay drill;
5. DynamoDB Streams shard behaviour and the EventBridge Pipe → SQS projection;
6. warm throughput at the 100/200/500 profiles;
7. every IAM allow/deny matrix, including the read-only projection role.

Item 3 deserves naming separately: the codec checks `expiresAt` explicitly and a
unit case proves the reader refuses an expired receipt, but "no fence anywhere
reads TTL" is a whole-system property that only a live soak can establish.

Even the local DynamoDB Local layer is currently unreachable — see the harness
gap in §4 — so the strongest evidence this stream has today is protocol-level:
the exact bytes of the request, and the exact participant a cancellation decodes
to.
