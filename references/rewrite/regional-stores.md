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

### `crates/aex-secret-custody-dynamodb`

The secret metadata, hidden source-generation, lineage, session-custody,
managed-call authorization, redaction-manifest and provider-credential row
families; the set, O(1) revoke, idle-only custody admission and only-before-
decrypt authorization transactions; and strongly consistent metadata/ciphertext
reads from deliberately separate partitions.

The metadata `Update` always writes `itemType="workspace_secret"`, including
when it creates the row. A set plan is rejected unless the target revision is
exactly one after the revision it claims to have observed, its metadata is
`ready`, and its sealed generation identity equals the metadata pointer. The
authority advances the stored revision itself with
`if_not_exists(revision, :zero) + :one`; it never trusts a caller-supplied
revision scalar.

37 default-lane tests and 6 DynamoDB Local integration tests pass. The engine
lane starts only through `aex_test_harness::DynamoDbLocalContainer`, so the
image comes exclusively from the harness's `images::reference` registry.

## 2. Deferred, with the reason

| Crate | State |
| --- | --- |
| `aex-content-dynamodb` | **not started.** §2.3 key templates, the SHA-256/BLAKE3 digest split, pins on roots, grant rows without a body copy, the GC epoch and the fenced sweep. |
| `aex-content-aws` | **not started.** §5 conditional create, multipart three-phase completion, presigned grants, `RedactedUrl`, fenced delete, `content_missing`. |
| `aex-registry-dynamodb` | **not started.** §2.4 pointer/ETag/upload state machine. |
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

---

# Regional stores, second wave — landed state

Branch `rw/regional-stores-2`, merged from `main` after the harness gained its
`containers` feature and after the regional pure domains landed. Nothing is
deployed, published or credentialed. Sections 1–7 above are the first wave and
are unchanged; everything below is the remaining seven crates.

## 8. Implemented

| Crate | What landed |
| --- | --- |
| `aex-content-dynamodb` | §2.3 in full: workspace-scoped body/root/tree/grant/GC keys, the SHA-256 body vs `BLAKE3` page digest split, root pins, grant rows that hold a reference and a pin, the GC epoch state machine and the fenced sweep. 58 unit + 7 engine cases. |
| `aex-content-aws` | §5 in full: `{workspace}/{2}/{2}/{sha256}` addressing, conditional create with the HEAD-compare resolution, the three-fact multipart integrity argument, presigned grants behind `RedactedUrl`, the fenced delete, the S3 error table, and the bucket-policy requirements as data. 52 unit + 7 engine cases. |
| `aex-registry-dynamodb` | §2.4 in full on `aex-workspace-domain`: pointer/`ETag`/revision, the upload state machine as conditions, native ordered listing with signed continuations, no index and no stream. 43 unit + 5 engine cases. |
| `aex-secret-custody-dynamodb` | §2.5 plus both additions: metadata and ciphertext in separate partitions, §3.6's four expressions verbatim, the `REDACT#{session}` manifest in its own partition, and the `pcr_` provider-credential directory. 36 unit + 5 engine cases. |
| `aex-secret-aws` | The OD-33 envelope arm: KMS root key → per-workspace branch key → per-message HKDF-SHA512 wrapping key → AES-256-GCM with the encryption context as AAD, a bounded zeroizing role-partitioned cache, and seal/rewrap/reveal. 39 unit + 3 engine cases. |
| `aex-secret-keystore-dynamodb` | §2.6: the provider schema verbatim, `KeyStoreBinding` with the logical name pinned to the physical table name, read-only introspection, and **no write path at all**. 24 unit + 3 engine cases. |
| `aex-runtime-activity-dynamodb` | §2.7 in full: generation head, lifecycle intents, immutable receipts, idle probes, the current-generation pointer and the 16-shard evaluation due index. 36 unit + 4 engine cases. |

Every crate now declares `layers = ["unit", "integration"]` with both covered:
288 unit cases and 34 engine-backed cases, none skipped.

### The engine-backed lane

`aex-test-harness`'s `containers` feature is wired into all seven crates as
`integration-engines = ["dep:tokio", "aex-test-harness/containers"]`, with one
`[[test]] name = "integration"` per crate behind `required-features`. Six run
against `DynamoDB` Local, one (`aex-content-aws`) against `MinIO`, and one
(`aex-secret-aws`) against `LocalStack`'s KMS. Five of the six `DynamoDB` targets
create their table from the checked-in `migrations/regional` definition rather
than from a shape the test invented, so the engine runs what Terraform creates.

The lane found four defects the protocol layer could not, each now fixed and
each with a case pinning it:

1. **Plan 05 §3.8's sweep transaction is illegal as written.** It carries a
   `ConditionCheck` on the descriptor *and* a `Delete` of that same descriptor;
   `DynamoDB` refuses two operations on one item and answers `ValidationException`
   before evaluating anything. The delete already carried the identical
   `gcEpoch = :markedEpoch` condition, so the check was redundant as well as
   illegal. `SWEEP_ORDER` is three participants, and the fence is unchanged.
2. **A row created by an `Update` never writes its own `itemType`.** The secret
   `set` writes metadata by `Update` because it carries a monotone revision the
   caller conditions on; the first `list_secrets` after a create then failed as
   `Missing { attribute: "itemType" }`. Any create-by-update elsewhere has the
   same hazard.
3. **An `INCLUDE` projection carries no discriminator**, so binding the shared
   typed row reader to a due-index row fails on every row. `runtime-activity`'s
   due scan now decodes the projection as the explicitly named slim set it is.
   **`aex_work_dynamodb::store::scan_due` has the identical `Row::bind` over
   `gsi_due` and will fail the same way** — see §11.
4. **An identical replay inside the ten-minute `ClientRequestToken` window is an
   idempotent success, not a lost condition.** Two cases had asserted a
   `PreconditionFailed` that the provider's transport deduplication never
   produces. They now assert the replay *and* that the same token with a changed
   payload is `IdempotencyConflict`.

## 9. Deferred, with the reason

| Item | State |
| --- | --- |
| `tests/live/aex-live-regional-stores/` | **Not created.** It would be a new workspace member, which this stream was told not to add, and `aex-workspace-check` additionally requires a live companion to name a `deployable` that is a member — this package would name none. Each of the seven crates instead points its `live_suite` at the existing companion for the deployable that exercises it (`aex-live-content-lifecycle-worker`, `aex-live-regional-session-api`, `aex-live-regional-secret-api`, `aex-live-regional-secret-key-admin`, `aex-live-runtime-control-worker`). Plan 05 §8.3's fourteen concerns belong in those packages; none of them is written, and all of them are blocked by OD-07 regardless. |
| `tests/load/regional-stores/` | Not written (plan 05 §8.4). |
| Merkle tree construction | The **rows** are here — tree pages, root descriptors, root pins, the GC scan index — but building, walking and copy-on-write persisting a tree is `aex-content-domain`'s, and plan 05 G-12's property burden lands there. |
| Deep-verify sampling | The descriptor carries `verifiedAt` and both checksums; the sampled re-hash job itself is `content-lifecycle-worker`'s. |
| `aex-content-aws` KMS client | The crate binds the encryption context through SSE-KMS headers and holds no KMS client. Application-layer AEAD for inline bodies and tree pages is `aex-secret-aws`'s envelope; there is exactly one crypto implementation. |

## 10. Published interfaces

- **`aex_content_dynamodb`** — `keys` (every `regional-content` template, `GC_PROJECTION`, `GC_BUCKETS`), `codec` (descriptor, inline body, pin, grant, root, tree page, GC epoch, GC candidate), `expressions` (`SWEEP_ORDER`, `sweep`, `mint_grant`, the epoch state machine, pin/unpin), `store::{ContentMetadataStore, ContentStore, Reachability, GcScanPage}`.
- **`aex_content_aws`** — `ObjectKey`, `PRESIGN_EXPIRY`, `MAX_SIGNATURE_AGE_MILLIS`, `RedactedUrl`, `BucketBinding`, `ContentObjectStore`, `S3ContentObjects`, `CompletionManifest`, `ContentObjectError`, and `policy::REQUIRED_DENIES` — the bucket-policy `Deny` statements this adapter depends on, as data the infrastructure stream can consume instead of re-deriving.
- **`aex_registry_dynamodb`** — `keys`, `codec` (built on `aex_workspace_domain::registry`/`upload`), `expressions` (pointer create/replace/delete, the upload transitions, completion begin/finish, consume), `store::{RegistryStore, RegistryDynamoStore, PointerPage}`.
- **`aex_secret_custody_dynamodb`** — `keys` (including `redaction_manifest` and `provider_credential`), `codec::{SecretMetadata, StoredGeneration, CustodyHead, CallAuthorization, RedactionManifest, ProviderCredential}`, `expressions::{set, revoke, admit_custody, authorize_managed_call, SET_ORDER, AUTHORIZE_ORDER}`, `store::{SecretCustodyStore, CustodyStore}`.
- **`aex_secret_aws`** — `context::{kms_pairs, aad_bytes, context_digest, name_digest}`, `envelope::{seal, open, header, BranchKeyMaterial, Entropy}`, `keystore::{BranchKeyProvider, KmsBranchKeys, BranchKeyCache}`, `crypto::{SecretCrypto, EnvelopeCrypto, SealedSecret, IMPLEMENTATION}`.
- **`aex_secret_keystore_dynamodb`** — `branch_key` (the provider record and its vocabulary), `store::{KeyStoreBinding, BranchKeyStoreReader, KeyStoreReader, ActiveBranchKey}`. `KeyStoreBinding` is what `regional-secret-key-admin` and `aex-secret-aws` both take.
- **`aex_runtime_activity_dynamodb`** — `keys` (templates, `DUE_PROJECTION`, `DUE_SHARDS`, the state spellings), `codec`, `expressions::{create_generation, transition, reschedule, record_intent, settle_intent, record_probe, point_current}`, `store::{RuntimeActivityStore, RuntimeActivityDynamoStore, DueGeneration}`.

## 11. Changes needed from peers

| Requirement | Owner |
| --- | --- |
| **`aex_work_dynamodb::store::scan_due` binds the typed row reader to a `gsi_due` row.** An `INCLUDE` projection carries the key attributes and the declared list and nothing else — not `itemType` — so `Row::bind` fails on every row and the reconciler returns `Corrupt` instead of a due page. `aex-runtime-activity-dynamodb` hit exactly this against `DynamoDB` Local and now decodes the projection directly; the same fix applies there. I did not edit the crate. | regional stores (first wave) |
| **`TransactionPlan` tokens can exceed the provider's 36-character `ClientRequestToken` ceiling.** `aex_session_dynamodb::transactions` builds tokens like `format!("terminal:{run}")`, which is 39 characters for a 30-character `RunId`. My crates hash instead (`token(tag, parts)` → `tag-` plus 32 hex). Unverified against a live service — `DynamoDB` Local accepts the long form — but the AWS API reference pins the field at 1–36. | regional stores (first wave) |
| **`aex_secret_domain::EncryptionContext::canonical_pairs` carries `aex:name` in the clear.** That is right for an in-process identity and wrong for KMS: an encryption context is authenticated *and* recorded in `CloudTrail`, and a customer-chosen secret name has no business in an audit log the customer cannot redact (D-17). `aex_secret_aws::context::kms_pairs` therefore substitutes `aex:name-digest` on the way out, salted with the workspace. The domain should either adopt the substitution or state that its context is never sent to a provider verbatim. | regional domains |
| The seven adapters define their own port traits marked `TODO(cross-stream)`. Where a peer trait now exists — `aex_content_domain`, `aex_workspace_domain`, `aex_secret_domain`, `aex_runtime_control` — the **data types are already the peers'** and only the trait needs re-pointing at merge. `ContentMetadataStore`, `ContentObjectStore`, `RegistryStore`, `SecretCustodyStore`, `SecretCrypto`, `BranchKeyStoreReader` and `RuntimeActivityStore` are the seven. | regional domains, runtime control |
| `aex-content-dynamodb` still carries `wire_pending::{Blake3Digest, SealedBytes, PinOwner, GrantPlan, GcSweepPlan}`. `aex_content_domain` now has `digest::PageDigest`, `pin::Pin`/`PinSubject` and `gc::SweepCandidate`, which are richer; the swap is mechanical but changes the key builders' signatures, so it is left as one deliberate edit rather than a rushed one. | regional stores + regional domains |
| The content bucket policy must carry the three `Deny` statements in `aex_content_aws::policy::REQUIRED_DENIES` verbatim, including `s3:signatureAge > 300000`. The adapter signs for exactly 300 seconds and a case pins the two values together. | infrastructure |
| The content and secret CMK policies still owe the `StringEquals` conditions on `kms:EncryptionContext:aex:workspace` and `aex:domain` (OD-18). `aex-secret-aws` sends the context on every `Decrypt` and `aex-content-aws` sends it on every SSE-KMS write; neither can enforce the condition. | infrastructure |
| `regional-secret-key-admin` owns `CreateKey`/`VersionKey` and every write to `regional-secret-keystore`. It should consume `KeyStoreBinding` rather than address the table itself. | regional services |
| `regional-otlp` reads the redaction manifest at `REDACT#{session_id}` / `MANIFEST` with `dynamodb:GetItem` and holds nothing else on that table. The manifest's HMAC algorithm and key id are row fields, so a rotation is explicit; the collector must compare digests rather than values. | observations |

## 12. Decisions taken

| # | Decision | Rationale |
| --- | --- | --- |
| RS-16 | Plan 05 §3.8's sweep is **three** actions, not four: the descriptor's `ConditionCheck` is dropped and its `Delete` carries the same condition. | `DynamoDB` rejects two operations on one item outright. The check was redundant anyway, so the fence is unchanged. |
| RS-17 | A `ClientRequestToken` is a fixed-width digest of the operation identity (`tag-` plus 32 hex), never a concatenation of identifiers. | Two thirty-character identifiers already overflow the 36-character field. Hashing makes the token width independent of its inputs. |
| RS-18 | The KMS-visible encryption context substitutes `aex:name-digest` for the domain's `aex:name`, salted with the workspace. | D-17. The context is recorded in `CloudTrail`; the salt additionally stops a reader correlating tenants by naming convention. |
| RS-19 | A generation's ciphertext lives at `SECGEN#{workspace}#{name}` / `GEN#{generation:020}` rather than plan 05's `SECGEN#{workspace}#{source_generation_id}`. | The domain's `SourceGeneration` is a per-`(workspace, name)` `u64`, not a global id, so the name has to be in the key. The property plan 05 wanted — ciphertext out of the partition a list reads — is preserved, and the sort key now gives an ordered lineage for free. |
| RS-20 | A grant pin declares `pinKind = "grant"`, outside `PIN_KINDS`. | It is addressed by `GRANT#{token}` rather than `PIN#{kind}#{id}` and is the only pin that expires, so folding it into the `PIN#` vocabulary would blur both facts. |
| RS-21 | `aex-content-aws` holds **no** KMS client. The encryption context is bound through SSE-KMS headers; application-layer AEAD is `aex-secret-aws`'s envelope. | One crypto implementation in the workspace. The crate keeps its `aws.kms.encryption_context` seam because it is what sets the context. |
| RS-22 | The presign lifetime and the `s3:signatureAge` deny are both 300 000 ms, and a case asserts they are equal. | OD-17 overrides plan 05 §5.5's six-minute proposal (G-7). Pinning them in one place makes the pair a test rather than a comment. |
| RS-23 | The branch-key version is derived from the wrapped bytes (`sha256(wrapped)[..16]`), not stored beside them. | A stored row cannot then claim a version its material does not have, and a rotation is detectable from the ciphertext alone. |
| RS-24 | `BranchKeyCache` is keyed by `(partition, branch key, version)` and evicts on a bound with no LRU accounting. | The bound is the security property; which entry goes is not. A cache miss costs one KMS call. |
| RS-25 | Registry and secret names are `aex_wire::ids::ResourceName`, whose grammar is ASCII `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`. | Plan 05 asks for "the canonical NFC name". An ASCII grammar is NFC by construction, so no normalization crate is needed and none was added. |
| RS-26 | The stored registry `ETag` is recomputed from `(kind, revision, digest)` on every read and compared. | The tag is a pure function of the row; a stored copy that disagrees describes a value that no longer exists, and handing a client that tag would make its next `If-Match` meaningless. |
| RS-27 | `aex-secret-keystore-dynamodb` decodes without the shared typed row reader. | The provider's schema has no `itemType`. Fabricating one would make the store unreadable by the provider's own tooling, which is the entire risk D-20 exists to avoid. |
| RS-28 | The redaction manifest lives at `REDACT#{session}` / `MANIFEST`, in its own partition. | `regional-otlp` holds `dynamodb:GetItem` and nothing else on the custody table. A manifest inside `CUSTODY#{session}` would be one key-guess away from a custody row; in its own partition, one point read is all the grant can reach. |
| RS-29 | A metadata row, a manifest and a credential binding all **refuse to decode** if they carry `ciphertext` or `wrappedKey`. | The types have nowhere to put sealed bytes, but a row written by hand or by an older revision could still have them. Refusing on read is what stops that becoming a leak on the list path. |
| RS-30 | `tests/live/aex-live-regional-stores/` was not created. | It is a new workspace member, which this stream was told not to add, and the registry additionally requires a live companion to name a member deployable. The concerns are recorded against the existing companions instead. |

## 13. Gate output

Run on `rw/regional-stores-2` at the final commit, `CARGO_BUILD_JOBS=4`:

```
cargo fmt --all                                              clean
cargo clippy -p aex-content-dynamodb -p aex-content-aws \
  -p aex-registry-dynamodb -p aex-secret-custody-dynamodb \
  -p aex-secret-keystore-dynamodb -p aex-secret-aws \
  -p aex-runtime-activity-dynamodb --all-targets -- -D warnings
                                                             clean
cargo nextest run -p aex-content-dynamodb -p aex-content-aws \
  -p aex-registry-dynamodb -p aex-secret-custody-dynamodb \
  -p aex-secret-keystore-dynamodb -p aex-secret-aws \
  -p aex-runtime-activity-dynamodb
                                        288 tests run: 288 passed, 0 skipped
cargo nextest run <the same seven> --features integration-engines \
  --profile integration -E 'binary(integration)'
                                         34 tests run:  34 passed, 0 skipped
cargo check --workspace --all-targets                        clean
cargo run -p aex-workspace-check         133 member(s) and 139 package(s)
                                         satisfy every structural and registry rule
cargo run -p aex-workspace-check -- registry build
                                         wrote release/test-registry.json and
                                         release/unearned-evidence.json (committed)
```

No `#[ignore]`, no environment self-skip, no empty suite, no retry-to-green. The
engine lane starts real containers from the digest-pinned registry through
`aex_test_harness::containers` and never names an image.

## 14. What could only be asserted locally

The engine lane raises the floor: expressions now provably parse and evaluate,
`INCLUDE` projections provably return what they declare and nothing more, sparse
indexes provably drop rows on a terminal transition, conditional writes provably
lose, and a real KMS round-trips a wrapped branch key under an enforced
encryption context. That is a materially stronger claim than the first wave
could make.

What the emulators cannot reach, stated rather than assumed:

1. **`MinIO` evaluates no delete precondition.** It accepts a `DeleteObject`
   whose `If-Match` does not match and answers `204` for an absent key, so
   `FencedDeleteOutcome::Changed` and `AlreadyAbsent` are unreachable there. The
   adapter's half — that it always sends the header — is asserted on the
   serialized request; that the *service* refuses a delete without one is a
   bucket-policy fact and is live-only. A case in the target says so out loud.
2. **`MinIO` implements no SSE-KMS**, so `put_immutable` cannot run against it.
   That gap is turned into a positive case instead: the adapter must refuse
   rather than quietly store a body unencrypted, and it must leave no object
   behind.
3. **`LocalStack` implements no KMS key policy**, so OD-18's
   `kms:EncryptionContext:aex:workspace` condition — the thing that makes the
   binding enforceable at the key rather than advisory — is unproved. What *is*
   proved there is that the context is carried and that a wrapped key presented
   under another workspace's context is refused.
4. **`DynamoDB` Local implements no TTL expiry**, no adaptive capacity, no
   `TransactionConflict` under real contention, and no Streams. Every TTL claim
   in these crates is therefore still "the reader checks `expiresAt` explicitly",
   asserted at the unit layer, plus "no fence reads TTL", which only a soak can
   establish.
5. **No AWS checksum semantics anywhere.** `MpuObjectSize`, `COMPOSITE` versus
   `CRC64NVME` and `EntityTooSmall` boundaries are protocol-asserted only.
6. Bucket-policy denial, `s3:signatureAge`, presigned-URL expiry behaviour, PITR
   and restore, real throttling shapes, and every IAM allow/deny matrix remain
   plan 05 §8.3 items with no local proxy.
The secret-custody review in §8 now reaches DynamoDB Local through the shared
harness. That closes local transaction and rollback evidence for the reviewed
set/replay path; it does not reduce any of the real-AWS gaps above.

## 8. Secret-custody review continuation

Branch `rw/regional-stores-review` was created from `4cf88eca`, merged with
`main` at `54c2d572` in `112992ba`, and repaired in `8947d4aa`. The active
`rw/regional-stores-2` worktree was read only for its status and was never
modified, merged, committed, or used as a source of uncommitted code.

The review separates two contracts that the earlier engine case had combined
under the misleading name
`a_second_set_at_the_revision_nobody_read_is_refused`:

- Plan 05 §3 makes `ClientRequestToken` provider transport deduplication for ten
  minutes. An identical request with the identical token therefore returns the
  first successful result. Plan 05 §7 reserves
  `IdempotentParameterMismatchException` for the same token with a different
  payload; that remains `StoreError::IdempotencyConflict`.
- A distinct set that claims it observed revision 7 targets revision 8 and is
  evaluated normally. Against a revision-1 record it fails as
  `StoreError::PreconditionFailed { participant: secret.metadata }`; the
  metadata remains byte-for-byte unchanged and the transaction leaves no
  generation-2 row. Identical replay success therefore does not weaken the
  stale-write fence or transaction atomicity.

Test-first evidence: the strengthened request-shape and invalid-plan cases
failed against the inherited implementation (15 passed, 2 failed) because it
emitted `SET revision = :revision` and accepted revision jumps. After the
authority-side increment and structural plan validation:

```
cargo clippy -p aex-secret-custody-dynamodb --all-targets --all-features -- -D warnings
                                                                clean
cargo nextest run -p aex-secret-custody-dynamodb                 37 passed, 0 skipped
cargo nextest run -p aex-secret-custody-dynamodb \
  --features integration-engines --test integration              6 passed, 0 skipped
cargo check --workspace --all-targets                            clean
cargo run -p aex-workspace-check                                 133 members / 139 packages clean
```
