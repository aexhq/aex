---
title: Historical central-finance Rust-native rewrite handoff
description: Point-in-time record of the superseded central-finance rewrite implementation and its former paths.
status: superseded historical record — session-plane clean cut 2026-08-14
keywords:
  - finance
  - ledger
  - rating
  - stripe
  - schema administration
audience: implementation agents and maintainers
last_verified: 2026-08-03
related:
  - references/rewrite/contracts.md
  - references/rewrite/usage.md
---

# Central-finance rewrite handoff

> **Superseded (2026-08-14).** This point-in-time rewrite record deliberately
> names finance packages, Stripe edges, schema tooling, rate authorities, and
> release units that the session-plane clean cut later removed or replaced. It
> is retained for provenance, not as current implementation guidance. The
> current public launch surface is the essential prepaid billing slice in
> [`../architecture.md`](../architecture.md); generated routes and
> `release/units.toml` are the executable authorities.

Plan of record: `references/rust-native-rewrite-2026-07-31/plans/03-central-finance-schema.md` in the parent workspace.

Branch: `rw/central-finance`

## Implemented

- `aex-finance-domain`: bounded integer cents/micro-USD types, the closed chart of accounts,
  debit-positive balanced transaction smart construction, immutable exact reversal, canonical
  intent hashing through `aex_wire::canonical`, reservation/settlement guards, billing admission,
  auto-top-up fences, and the provider effect state/recovery machine. A provider 5xx cannot become
  `Failed`; unknown recovery changes from exact replay to lookup after 12 hours.
- `aex-usage-rating`: verified immutable signed rate books, all four exact `BigRational` quanta,
  exact accumulation, a single rational-to-micro-USD rounding site, deterministic largest-remainder
  allocation, golden invoice/property/replay tests, and shadow-only `M-STOR-CLOSE` receipts. Interior
  storage closes floor, hard-delete closes ceil, and every receipt carries an asserted exclusive
  `bytes * one minute` error bound.
- `aex-finance-app`: typed FIFO rating requests. `MessageGroupId` is exactly the organization id;
  deduplication is `<region>:<category>:<factId>`; partial-batch failures name only facts or
  organization groups that did not commit.
- `aex-finance-aurora`: strict integer/string Data API row decoding, no floating-value accessor,
  64 KiB row and 1 MiB response rejection, database config validation, finance SQL constants, and
  business-key resolution for a lost commit response.
- `migrations/central`: one linear ten-file chain from `20260801000000` through
  `20260801000900`, preserving the merged peer bootstrap/identity/control/control-functions bodies
  before the finance roles/schema/DDL/seed bodies,
  declarative `grants.toml`, immutable journal tables, a deferred balanced-transaction constraint
  trigger, reversal-only mutation guard, customer prepaid balance fence, finance inbox/outbox/effect,
  usage/rating/statement/provider-cost tables, roles, and the public `synthetic-zero-v1` seed.
- `central-schema-admin`: exact one-shot clap command tree, stable exit codes, native SQLx `Migrator`
  configured for `schema_admin._sqlx_migrations`, compile-time embedded migration SQL, bundle lock,
  and grants allowlist, migration header/linearity validation, declarative grant validation, and
  local canonical plan receipts using advisory lock
  `0x4145585F4D494752` (`4703262552200136530`).
- `finance-api`: total no-default configuration, role/plane validation, Lambda/axum composition, and
  distinct `/internal/healthz` and `/internal/readyz` behavior. Readiness remains false until the
  deployable-specific database-role probe exists.
- `stripe-command-edge`: exact `stripe@22.4.0`, API `2026-06-24.dahlia`, timeout 8000 ms,
  `maxNetworkRetries: 0`, one four-field effect metadata vocabulary, six generated command variants,
  the finance-authored idempotency key, strict deadline/config validation, Secrets Manager loading,
  and total provider failure classification. A 5xx, timeout, reset, rate limit, or ambiguous failure
  is never a determinate rejection.
- Payment recovery: `finance-api` binds the complete admitted `PaymentCommandEnvelope` to the
  prepared effect before invoking Stripe. `finance-reconcile` keyset-pages unresolved effects using
  a durable cursor, exact-key replays the original command inside the replay window, then performs a
  direct-or-metadata PaymentIntent lookup outside it. Both recovery and escalation are fenced by the
  effect revision, so a concurrent webhook wins instead of being overwritten. The command edge now
  returns the Rust `PaymentResult` wire shape and treats only a succeeded PaymentIntent as success;
  transport loss, provider 5xx, and nonterminal provider states remain `Unknown`.
- `stripe-webhook-edge`: exact raw/base64 bytes, 256 KiB rejection before signature work, bounded
  current/previous secret rotation, SDK signature verification before payload interpretation, the
  exact eight-event allowlist, API-version quarantine, a scalar-only PII-free fact projection, raw
  SHA-256, synchronous Lambda invocation, and 2xx only after finance ingest accepts the event.
- Test architecture: real mapped unit/integration/smoke/e2e evidence targets for every implemented
  Rust package, complete AEX metadata for both TypeScript edges, Stripe companion seam claims, no
  skips, regenerated `release/test-registry.json` and `release/unearned-evidence.json`.

## Remaining gaps

The recovered composition pass replaced every finance deployable's fail-fast shell and gave
`central-schema-admin` re-entrant online execution. Its grant reconciler now probes and applies the
whole schema-v2 allowlist: database `CONNECT`, schema `USAGE`, function `EXECUTE`, table/view
privileges, and the declared `PUBLIC` revocations. The exact deployable behavior and the narrower
remaining implementation gaps are recorded in [Composition](#composition).

The following work still depends on peers or deployment:

- PaymentIntent recovery has exact direct-object and metadata-search authority. Customer, Checkout
  Session, Portal Session, and Refund recovery remains deliberately unsupported: the reconciler
  escalates these kinds after their exact replay window instead of calling an incompatible provider
  API or guessing an outcome.
- The webhook edge produces the accepted eight-event bounded envelope, but the landed generated Rust
  payment event contract represents only five events; it cannot yet be consumed byte-for-byte by
  `finance-ingest`.
- All nine live companions remain unearned evidence because no remote Stripe, AWS, Aurora, queue,
  IAM, load, or soak evidence can be earned before deployment.
- Direct-invoke Rust Lambdas expose invocation probes rather than HTTP health routes. Resource shapes
  live in the sanctioned `[unit.lambda]` and `[unit.fargate]` rows of `release/units.toml`.

## Published peer types

Every new Rust type intended as a peer surface is listed by exact path:

- `aex_finance_domain::account::{Currency, AccountSide, AccountKind, AccountRef}`
- `aex_finance_domain::billing_account::{BillingAccountState, BillingAccount, AdmissionDecision,
  BlockReason, AutoTopUpHistory, AutoTopUpDecision, AutoTopUpBlock}`
- `aex_finance_domain::effect::{EffectKind, EffectState, ProviderObject, DeclineCode,
  IndeterminateReason, EffectOutcome, ProviderEffect, RecoveryAction, EffectTransitionError}`
- `aex_finance_domain::journal::{TransactionId, TransactionKind, BusinessKey, BusinessKeyError,
  IntentHash, Posting, BalancedTransaction, ConservationError}`
- `aex_finance_domain::money::{MoneyError, Cents, Microusd, MicrousdDelta}`
- `aex_finance_domain::reservation::{ReservationState, Closure, Reservation, ReservationError}`
- `aex_finance_domain::transitions::TransitionIdentity`
- `aex_usage_rating::exact::{RateBookId, Contribution, SegmentKey, RatedSegment}`
- `aex_usage_rating::rate_card::{PlaneBilling, RoundingRule, Rate, BookVerifier, RateBook,
  RateContext, RatingError}`
- `aex_usage_rating::storage_close::{StorageCloseKind, ShadowStorageClose, StorageCloseError}`
- `aex_finance_app::use_cases::{RatingRequest, FifoRatingMessage, FactFailure}`
- `aex_finance_aurora::row::{RowPage, RowDecodeError}`
- `aex_finance_aurora::store::{FinanceDbConfig, FinanceAuroraConfigError}`
- `aex_finance_aurora::tx::{PostedReceipt, UnknownCommit, CommitDisposition, CommitProbe,
  CommitResolution, UnknownCommitError}`

Temporary cross-stream types, all carrying the required replacement comment:

- `services/stripe-command-edge/src/wire_pending.ts::PaymentCommandEnvelope` ->
  `aex-payment-contracts::PaymentCommandEnvelope`
- `services/stripe-command-edge/src/wire_pending.ts::PaymentCommand` ->
  `aex-payment-contracts::PaymentCommand`
- `services/stripe-command-edge/src/wire_pending.ts::PaymentCommandFailure` ->
  `aex-payment-contracts::PaymentCommandResult`
- `services/stripe-webhook-edge/src/wire_pending.ts::ProviderEventEnvelope` ->
  `aex-payment-contracts::ProviderEventEnvelope`

## Changes needed from peers

1. **Settled.** Central identity published `aex_rds_data` with a different and stronger shape than
   this plan asked for, and the Aurora adapter was adapted to it rather than the other way round.
   A failed commit is `aex_rds_data::CommitFailure`, an enum whose `RolledBack` arm states nothing
   was applied and whose `Unknown` arm is the lost response; there is no `CommitOutcomeUnknown`
   struct. Rows are read through `aex_rds_data::Record`'s indexed accessors, which have no
   floating-point decode path at all, rather than through a `FieldValue` enum; there is no
   `aex_rds_data::row` module. `aex_finance_aurora::wire_pending` is deleted:
   `CommitDisposition::classify` is now the one place a lost commit becomes the explicit
   `UnknownCommit` that `resolve_unknown_commit` requires, and money columns decode from
   `Record::i64` because `amount_microusd` and `balance_microusd` are `bigint`.
2. Contracts must generate TypeScript payment command/result/event types. The event contract must
   reconcile its current five variants with the binding's eight exact event strings by adding
   `payment_intent.canceled`, `charge.dispute.closed`, and `refund.updated` plus their closed facts.
3. Infrastructure/delivery must choose the policy-valid source for Lambda memory, timeout and
   reserved concurrency and configure the Stripe endpoint event list equal to
   `HANDLED_EVENT_TYPES`.
4. The observation/usage producer should consume `ShadowStorageClose` so the required rounding
   error/bound travels in every shadow close; billing activation remains fenced on METER-03 signoff.

## Decisions and resolved specification conflicts

- The binding's storage rate `5 / 940_597_837_824` micro-USD per byte-minute is authoritative. It
  rates 1 GiB-month to **250 micro-USD**, while plan test prose RT10 says `250000`. Tests assert 250;
  changing it would violate the explicit quantum. **Reversed 2026-08-06:** this resolved the
  conflict the wrong way. `940_597_837_824 = 1 GiB x 876`, so a numerator of `5` rates a GiB-month
  (43,800 minutes = 50 x 876) to 250 micro-USD, i.e. $0.00025 - a factor of 1000 below the accepted
  A11-PRICING rate of $0.25/GiB-month. RT10's `250000` was right. The other three quanta were already
  exact ($0.25/vCPU-hour, $0.03/GiB-hour, $0.30/GB), which is what makes this a typo rather than a
  design disagreement. The numerator is now `5000`; the golden quanta table and the RT10 insta
  snapshot are re-baselined. Tests that encode a defect are not evidence for it.
- `finance.pricing_context` was declared and never populated, so its two foreign keys —
  `finance.reservation.pricing_version` and `finance.usage_inbox.pricing_version` — pointed at an
  empty table and no reservation or usage fact could be stored at all. `20260801000900` seeds one
  row and one only: `synthetic-zero-v1`, all four meters at zero, `billingActive = false`, rounding
  `half_even`, effective from the epoch and unbounded. A migration is the declared writer because
  `grants.toml` gives no role `INSERT` on the table.
  The priced launch card is deliberately **not** seeded and cannot be seeded from this repository.
  OD-09 keeps commercial values out of public source; a central migration applies byte-identically
  to dev and prd, so it cannot express a plane-specific card; and `pricing_context.signature` has no
  producer, because no rate-book signing key and no production `BookVerifier` exist. The seeded row
  therefore carries the text `unsigned:synthetic-zero-v1`, which states the absence rather than
  standing in for a signature, and `aex-usage-rating` asserts that a verifier which rejects it still
  fails the open and that an active plane refuses the book outright. Installing a priced context
  needs three decisions this repository cannot make: the accepted rate values, a signing key and its
  binding, and the mechanism by which a private book reaches a plane's database.
- The binding's advisory lock hex is authoritative. It evaluates to
  `4703262552200136530`; a different decimal (`4703167197722708306`) in plan prose was not used.
- The generated payment contract's six commands were implemented at the TypeScript boundary rather
  than inventing the plan prose's older nine-command shape.
- Merged central-identity DDL arrived as four-digit `0001` through `0004` filenames, which violate
  the binding runner format and would be interpreted by SQLx as a second incompatible version line.
  Under the runner owner's merge authority they were mechanically renamed, without reordering or
  rewriting their SQL bodies, to `20260801000000` through `20260801000300`. Finance follows at
  `20260801000400` through `20260801000600`.
- Stripe 22.4.0's declaration narrows `apiVersion` to its package-latest
  `2026-07-29.dahlia`. Runtime Stripe accepts an older explicit Dahlia pin, so the constructor uses a
  type-only cast while the actual value remains `2026-06-24.dahlia` and is snapshot-tested.
- No network retry, canonicalizer, float money/quantity, default resource identifier, credential
  read, provider call, AWS call, deployment, publish, or compatibility shim was introduced.

## Commits

- `e246d0d3` `feat(finance): enforce balanced integer money authority`
- `66934c87` `feat(finance): add exact rational usage rating`
- `e65c8220` `feat(finance): add guarded central schema baseline`
- `31f705dd` `feat(finance): harden Aurora money boundaries`
- `fdf40beb` `feat(finance): pin FIFO settlement identities`
- `93478d41` `feat(finance): compose strict finance API health surface`
- `859dc095` `feat(finance): add pinned Stripe protocol edges`
- `07e05fed` `test(finance): map implementation evidence targets`
- `138b80d3` `chore(test): regenerate finance evidence registry`
- `e0fcf3ad` `feat(finance): report bounded storage close rounding`

## Verification

The final gate commands and their verbatim terminal output are reported in the implementation-agent
handoff message. This file is intentionally kept stable rather than embedding machine-specific build
timings.

---

# Composition

Branch: `rw/deploy-finance`, off `main` after the four-stream merge. This pass
replaces the typed `NotImplemented` in every finance deployable's `run()` with a
real composition root. The seven binaries now build their adapters, prove their
own database grants, and enter their real runtime.

## What each deployable now does

| Deployable | Entry | What `run()` does now |
| --- | --- | --- |
| `services/finance-api` | `lambda_http::run` | Mounts every route of `RouteGroup::Billing` by iterating the generated table, dispatches through `dispatch_billing`, and serves `/internal/healthz` and `/internal/readyz`. Builds the Aurora billing authority, the Stripe command-edge gateway and the S3 statement presigner. |
| `services/finance-ingest` | `lambda_runtime::run` | One serializable transaction claims the provider event on `provider_event_id`, posts the balanced transition it implies, and binds the transaction to the inbox row. Answers success only after that commit. |
| `workers/finance-settlement-worker` | `lambda_runtime::run` over `SqsEventObj` | Partitions the batch into one group per organization, settles each group in one serializable transaction, and returns a partial-batch response naming only uncommitted messages. |
| `workers/finance-reconcile` | `lambda_runtime::run` | Runs the `R-BAL` and `R-SUM` conservation sweeps and the unresolved-effect sweep, escalates an effect past its replay window to `manual_review`, and publishes every finding to the operations topic. |
| `workers/usage-receipt-dispatcher` | `lambda_runtime::run` | Reads one pending page per category and replays it to the regional receipt queue, retiring a receipt only after the queue accepted it. |
| `workers/provider-cost-reconciler` | `lambda_runtime::run` | Reads one bounded provider cost export, normalises it on `(source, source_row_id)`, and reports the period margin in exact integer basis points. |
| `workers/central-schema-admin` | `clap` one-shot | Resolves the DDL credential, opens a `verify-full` session against the pinned CA bundle, takes the outer advisory lock without waiting, and runs `plan`/`migrate`/`verify`/`grants`/`backfill`/`repair` under the numbered exit contract. |

## Composition decisions

| # | Decision | Rationale |
| --- | --- | --- |
| D-01 | Every deployable reads its configuration under the namespace `release/units.toml` already registers for it (`AEX_FINANCE_API_`, `AEX_FINANCE_INGEST_`, …), replacing the earlier unnamespaced `AEX_AURORA_*` variables | Two deployables on one plane share an environment; an unnamespaced cluster ARN makes a wrong binding silent. The registry already named the namespace and nothing read it. |
| D-02 | Readiness is a **grant** fact, not a liveness fact: each startup probe asserts the process holds its own role, can reach the relations it needs, and **cannot** reach the ones it must not | A grant table nobody connects as is a document. `finance-api` refuses to report ready if its role holds `UPDATE` on `finance.journal_transaction`; `provider-cost-reconciler` refuses to start at all if it can reach a customer posting (F-26). |
| D-03 | The Lambda and Fargate resource shapes are declared in `release/units.toml` `[unit.lambda]` / `[unit.fargate]`, not in `[package.metadata.aex]` | `graph verify` reads `release/units.toml`; the `[package.metadata.aex]` schema is a closed thirteen-key vocabulary that rejects them, which is what the previous handoff recorded as blocked. This resolves that block by using the location the tool actually checks. |
| D-04 | `finance-ingest` no longer declares `health_path` / `ready_path` in `release/units.toml`; its probes are `{"request":"healthz"}` and `{"request":"readyz"}` invoke arms | It is a direct-invoke Lambda behind `stripe-webhook-edge` and serves no HTTP. A declared HTTP path nothing answers is drift. |
| D-05 | `finance-api` mounts against a one-method `CentralEdge` trait it owns, and composes `UnresolvedPrincipalEdge`, which refuses every request with `unauthenticated` | `aex-central-http`'s router is still a documentation stub and is owned by a parallel stream. Inventing a principal in a money authority is the worst available defect, so the surface fails closed. Every route is mounted and reachable; only the principal is unresolved. |
| D-06 | A per-organization `finance.account` identity is **derived** (`blake3` over organization and account kind, stamped to a v7 shape) rather than minted | Two concurrent first postings for one account converge on one row instead of racing to two. |
| D-07 | `CommandKind::CreatePortalSession` prepares no durable `provider_effect` row | `finance.provider_effect.kind` has no `portal_session` value and a hosted portal page creates no money effect. The derived provider idempotency key still fences a duplicate object inside the provider's replay window. Recorded rather than worked around by widening the DDL enum. |
| D-08 | Only a serialization race is retried inside a settlement invocation; an unknown commit outcome goes back to the queue | The transaction body is replay-safe with no external effect, so a `40001` retry is free. An unknown outcome must be resolved against durable state on redelivery, not by a second attempt in the same process. |
| D-09 | `aws-sdk-lambda` (1.138.0) and `aws-sdk-sns` (1.107.0) were added to `[workspace.dependencies]` | The only Rust-to-Stripe path is a synchronous invoke of `stripe-command-edge`, and the sweeps report through the operations topic. Both are in the plan's permission model and neither had a workspace entry. |
| D-10 | `finance-settlement-worker` declares seam `aws.sqs.redrive` rather than `aws.sqs.partial_batch` | `aws.sqs.partial_batch` is not in `release/policy/seams.toml`; the registered redrive seam covers the same behaviour. Adding a seam row is the seam registry owner's call. |

## Environment variables, per deployable

Every variable below is **required**; there is no default for any of them, and
start-up names the first one that is absent or unusable.

### `finance-api` (12)

```
AEX_FINANCE_API_PLANE                     dev | prd
AEX_FINANCE_API_REGION                    AWS region name
AEX_FINANCE_API_AURORA_CLUSTER_ARN        arn:aws:rds:...:cluster:...
AEX_FINANCE_API_AURORA_SECRET_ARN         arn:aws:secretsmanager:...
AEX_FINANCE_API_DATABASE_NAME             logical database
AEX_FINANCE_API_DATABASE_ROLE             must equal `aex_finance_api`
AEX_FINANCE_API_STRIPE_COMMAND_EDGE_ARN   arn:aws:lambda:...
AEX_FINANCE_API_DEFAULT_PRICING_VERSION   e.g. `synthetic-zero-v1`
AEX_FINANCE_API_STATEMENT_BUCKET          statement artifact bucket
AEX_FINANCE_API_TX_DEADLINE_MS            positive integer
AEX_FINANCE_API_PAGE_LIMIT                1..=1000
AEX_FINANCE_API_DOWNLOAD_GRANT_TTL_MS     1..=300000  (OD-17)
```

### `finance-ingest` (8)

```
AEX_FINANCE_INGEST_PLANE                        dev | prd
AEX_FINANCE_INGEST_REGION                       AWS region name
AEX_FINANCE_INGEST_AURORA_CLUSTER_ARN           arn:aws:rds:...
AEX_FINANCE_INGEST_AURORA_SECRET_ARN            arn:aws:secretsmanager:...
AEX_FINANCE_INGEST_DATABASE_NAME                logical database
AEX_FINANCE_INGEST_DATABASE_ROLE                must equal `aex_finance_ingest`
AEX_FINANCE_INGEST_PINNED_STRIPE_API_VERSION    `2026-06-24.dahlia`
AEX_FINANCE_INGEST_TX_DEADLINE_MS               positive integer
```

### `finance-settlement-worker` (10)

```
AEX_FINANCE_SETTLEMENT_PLANE                     dev | prd
AEX_FINANCE_SETTLEMENT_REGION                    AWS region name
AEX_FINANCE_SETTLEMENT_AURORA_CLUSTER_ARN        arn:aws:rds:...
AEX_FINANCE_SETTLEMENT_AURORA_SECRET_ARN         arn:aws:secretsmanager:...
AEX_FINANCE_SETTLEMENT_DATABASE_NAME             logical database
AEX_FINANCE_SETTLEMENT_DATABASE_ROLE             must equal `aex_finance_settlement`
AEX_FINANCE_SETTLEMENT_QUEUE_URL                 must end `.fifo`  (F-10)
AEX_FINANCE_SETTLEMENT_MAX_GROUP_BATCH           1..=10000
AEX_FINANCE_SETTLEMENT_SERIALIZATION_RETRY_MAX   1..=3
AEX_FINANCE_SETTLEMENT_TX_DEADLINE_MS            1..=900000
```

### `finance-reconcile` (12)

```
AEX_FINANCE_RECONCILE_PLANE                                dev | prd
AEX_FINANCE_RECONCILE_REGION                               AWS region name
AEX_FINANCE_RECONCILE_AURORA_CLUSTER_ARN                   arn:aws:rds:...
AEX_FINANCE_RECONCILE_AURORA_SECRET_ARN                    arn:aws:secretsmanager:...
AEX_FINANCE_RECONCILE_DATABASE_NAME                        logical database
AEX_FINANCE_RECONCILE_DATABASE_ROLE                        must equal `aex_finance_reconcile`
AEX_FINANCE_RECONCILE_STRIPE_COMMAND_EDGE_ARN              arn:aws:lambda:...
AEX_FINANCE_RECONCILE_UNKNOWN_EFFECT_RETRY_WINDOW_HOURS    1..=12  (F-15)
AEX_FINANCE_RECONCILE_SWEEP_PAGE                           1..=10000
AEX_FINANCE_RECONCILE_ALARM_TOPIC_ARN                      arn:aws:sns:...
AEX_FINANCE_RECONCILE_STATEMENT_BUCKET                     statement artifact bucket
AEX_FINANCE_RECONCILE_TX_DEADLINE_MS                       1..=900000
```

### `usage-receipt-dispatcher` (12)

```
AEX_USAGE_RECEIPT_PLANE                  dev | prd
AEX_USAGE_RECEIPT_REGION                 AWS region name
AEX_USAGE_RECEIPT_AURORA_CLUSTER_ARN     arn:aws:rds:...
AEX_USAGE_RECEIPT_AURORA_SECRET_ARN      arn:aws:secretsmanager:...
AEX_USAGE_RECEIPT_DATABASE_NAME          logical database
AEX_USAGE_RECEIPT_DATABASE_ROLE          must equal `aex_receipt_dispatcher`
AEX_USAGE_RECEIPT_QUEUE_URL_COMPUTE      https://sqs...
AEX_USAGE_RECEIPT_QUEUE_URL_STORAGE      https://sqs...
AEX_USAGE_RECEIPT_QUEUE_URL_TRANSFER     https://sqs...
AEX_USAGE_RECEIPT_BATCH_SIZE             1..=10
AEX_USAGE_RECEIPT_MAX_ATTEMPTS           1..=100
AEX_USAGE_RECEIPT_TX_DEADLINE_MS         1..=900000
```

### `provider-cost-reconciler` (12)

```
AEX_PROVIDER_COST_PLANE                        dev | prd
AEX_PROVIDER_COST_REGION                       AWS region name
AEX_PROVIDER_COST_AURORA_CLUSTER_ARN           arn:aws:rds:...
AEX_PROVIDER_COST_AURORA_SECRET_ARN            arn:aws:secretsmanager:...
AEX_PROVIDER_COST_DATABASE_NAME                logical database
AEX_PROVIDER_COST_DATABASE_ROLE                must equal `aex_provider_cost`
AEX_PROVIDER_COST_CUR_BUCKET                   cost export bucket
AEX_PROVIDER_COST_CUR_PREFIX                   normalized object prefix
AEX_PROVIDER_COST_MARGIN_ALERT_THRESHOLD_BPS   0..=10000
AEX_PROVIDER_COST_ALARM_TOPIC_ARN              arn:aws:sns:...
AEX_PROVIDER_COST_MAX_SCAN_BYTES               positive integer
AEX_PROVIDER_COST_TX_DEADLINE_MS               1..=900000
```

### `central-schema-admin`

No environment variables: every input is an explicit CLI argument, so a
migration run is reproducible from its recorded argv alone. The one credential
is resolved from `--database-secret-arn` at run time and never enters argv,
output or a log line.

## Declared resource shapes

| Unit | Shape |
| --- | --- |
| `finance-api` | 512 MiB, 15 s, reserved concurrency 40 |
| `finance-ingest` | 512 MiB, 10 s, reserved concurrency 20 |
| `finance-settlement-worker` | 1024 MiB, 60 s, reserved concurrency 30 |
| `finance-reconcile` | 1024 MiB, 300 s, reserved concurrency 2 |
| `usage-receipt-dispatcher` | 512 MiB, 60 s, reserved concurrency 10 |
| `provider-cost-reconciler` | 2048 MiB, 900 s, reserved concurrency 1 |
| `central-schema-admin` | Fargate: 512 CPU, 1024 MiB, desired count 0, stop timeout 120 s |

The reservations are ordered, and the ordering is asserted by test:
`finance-reconcile` and `usage-receipt-dispatcher` are each strictly below the
lanes they depend on, so reconciliation and replay can never consume the budget
that produces the work they exist to repair (F-29).

## What is still owed

| Gap | Owner | Note |
| --- | --- | --- |
| `aex-central-http` has no router, so `finance-api` composes `UnresolvedPrincipalEdge` and answers `401` on every billing route | central identity/control | The seam is one method, `CentralEdge::admit`. Swapping it in is a wrapper, not a translation. |
| No billing route declares an unavailability error code, so an unreachable Aurora renders as `internal_error` through `dispatch::declared` | contracts | `finance-api` emits `account_state_unavailable` honestly; the gap closes the moment the route table admits it. |
| `finance-settlement-worker` durably claims inbox facts but cannot yet rate or post them | central finance plus admission/usage contracts | The missing authority is larger than a `RateContext` query: no production `BookVerifier` or trusted signing-key binding exists; `20260801000900` now seeds the shadow `pricing_context`, but no writer or seed creates a `reservation`, and `grants.toml` gives no role `INSERT` on `finance.reservation` to create one with; no durable exact segment accumulator or closure writer says when once-only rounding is complete; `UsageFact` carries no correction head; and a zero-book receipt requires a transaction id while zero journal postings are forbidden. The worker now keeps every `pending` or `quarantined` group in the SQS partial-batch response, so an intermediate inbox commit is never acknowledged as settlement. It will redrive and eventually DLQ while blocked; the pending row remains durable, but no central due-scan authority exists to wake it after the queue notification is gone. |
| `finance-reconcile` cannot yet invoke the command edge to resolve an unknown effect | central finance plus payment contracts | Exact-key replay is impossible from the current durable row: `request_json` stores only kind and amount, not the admitted command. The TypeScript edge still returns its temporary result shape rather than Rust `PaymentResult`; lookup searches PaymentIntents only and currently labels any found status successful. The reconciler therefore reports replayable, lookup-required and prepared/dispatched stranded effects instead of silently calling the sweep clean; it uses the configured replay window and escalates only paths for which no supported lookup exists. Its bounded unresolved-effect page has no durable cursor, so a permanent first page can still starve later effects until the recovery authority also owns cursor state. |
| The planned cross-plane schema-head release document is absent, so aex-release-tool migration verify has no declared cross-plane head to consume | central finance and regional stores | The central bundle is reproducible at `20260801000900`; the regional table document has a digest but no authoritative generation, so the head cannot be invented locally. |
| `migrations/central/20260801000000_bootstrap.sql` is the renamed central bootstrap, while the integration-only migration test still points at the former unversioned filename | central identity | The suite is behind `required-features = ["integration-engines"]`, so `cargo check --all-targets` does not build it and the breakage is invisible in the default lane. |
| `graph verify` reports 147 workspace-wide gaps: one live-companion metadata gap and 146 uncovered routes | test architecture and route owners | The finance composition does not add a new graph violation; these are the current global scenario-ownership debts. |
| Every `tests/live/aex-live-*` companion is still an unearned-evidence row | central finance | No live evidence can be earned before deployment (OD-07). |

## 2026-08-02 settlement/reconciliation continuation evidence

- `cargo test -p finance-settlement-worker -p finance-reconcile` — **66 passed**, zero failed:
  36 settlement tests and 30 reconciliation tests across unit, conformance, property,
  resource-envelope, smoke and doc-test targets.
- `cargo fmt -p finance-settlement-worker -p finance-reconcile -- --check` — clean.
- `cargo clippy -p finance-settlement-worker -p finance-reconcile --all-targets -- -D warnings`
  — clean.
- No live provider, Aurora, SQS, deployment or AWS operation was run. The blocked
  authority paths above therefore remain blocked rather than simulated or guessed.

## 2026-08-03 payment-recovery authority

- The prepared provider-effect row now contains the complete admitted payment command before the
  provider call begins. A lost database response is safe: rebinding succeeds only when the durable
  command is byte-for-byte equivalent under the same effect and intent hash.
- The reconciliation cursor advances only after a page is processed and wraps after the final page.
  An unresolved first page can therefore neither starve later effects nor be skipped after a failed
  sweep.
- Exact replay retains the effect id, intent, and provider idempotency key; only the transport
  deadline is renewed. Outside the replay window, only PaymentIntent effects use provider lookup.
  Unsupported object kinds escalate to `manual_review` rather than receiving guessed recovery.
- Every terminal recovery update includes the previously read revision and unresolved-state guard.
  A webhook or another sweep that commits first makes the recovery write a no-op.
- Focused verification covers command binding, envelope corruption rejection, replay identity,
  revision fences, cursor progression, exact TypeScript/Rust result parity, and failure ambiguity.
  No live provider, Aurora, deployment, or AWS operation was used to earn this implementation
  evidence.

## 2026-08-03 schema-admin image authority

- The production schema-admin path no longer resolves `CARGO_MANIFEST_DIR` at runtime. SQLx forward
  migrations, the generated bundle lock, every future explicit repair, and `grants.toml` are
  compile-time inputs to the ELF copied into the minimal OCI image.
- The artifact graph declares `bundle:migration` as a direct schema-admin input. Artifact description
  now fails closed if that closure lacks `migrations/central/bundle.lock.json`, and the emitted
  envelope carries its exact SHA-256 as `identities.migration.centralBundleDigest`.
- Central migration files are pinned to LF in `.gitattributes`. The bundle lock is therefore an
  operating-system-independent identity rather than a digest of whichever line endings a release
  runner checked out.
