---
title: Central-finance Rust-native rewrite handoff
description: Implemented authority, exact rating, central finance schema, provider edges, evidence, peer requests, and explicit deferred work on rw/central-finance.
status: accepted
keywords:
  - finance
  - ledger
  - rating
  - stripe
  - schema administration
audience: implementation agents and maintainers
last_verified: 2026-08-01
related:
  - references/rewrite/contracts.md
  - references/rewrite/usage.md
---

# Central-finance rewrite handoff

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
- `migrations/central`: one linear seven-file chain from `20260801000000` through
  `20260801000600`, preserving the merged peer bootstrap/identity/control/control-functions bodies
  before the finance roles/schema/DDL/seed bodies,
  declarative `grants.toml`, immutable journal tables, a deferred balanced-transaction constraint
  trigger, reversal-only mutation guard, customer prepaid balance fence, finance inbox/outbox/effect,
  usage/rating/statement/provider-cost tables, roles, and the public `synthetic-zero-v1` seed.
- `central-schema-admin`: exact one-shot clap command tree, stable exit codes, native SQLx `Migrator`
  configured for `schema_admin._sqlx_migrations`, migration header/linearity validation, declarative
  grant validation, and local canonical plan receipts using advisory lock
  `0x4145585F4D494752` (`4703262552200136530`).
- `finance-api`: total no-default configuration, role/plane validation, Lambda/axum composition, and
  distinct `/internal/healthz` and `/internal/readyz` behavior. Readiness remains false until the
  deployable-specific database-role probe exists.
- `stripe-command-edge`: exact `stripe@22.4.0`, API `2026-06-24.dahlia`, timeout 8000 ms,
  `maxNetworkRetries: 0`, one four-field effect metadata vocabulary, six generated command variants,
  the finance-authored idempotency key, strict deadline/config validation, Secrets Manager loading,
  and total provider failure classification. A 5xx, timeout, reset, rate limit, or ambiguous failure
  is never a determinate rejection.
- `stripe-webhook-edge`: exact raw/base64 bytes, 256 KiB rejection before signature work, bounded
  current/previous secret rotation, SDK signature verification before payload interpretation, the
  exact eight-event allowlist, API-version quarantine, a scalar-only PII-free fact projection, raw
  SHA-256, synchronous Lambda invocation, and 2xx only after finance ingest accepts the event.
- Test architecture: real mapped unit/integration/smoke/e2e evidence targets for every implemented
  Rust package, complete AEX metadata for both TypeScript edges, Stripe companion seam claims, no
  skips, regenerated `release/test-registry.json` and `release/unearned-evidence.json`.

## Deliberately deferred

Depth was landed in the plan's authority/rating/schema priority order. The following generated
composition shells remain fail-fast and retain their `not_applicable.targets` evidence debt:

- `services/finance-ingest`
- `workers/finance-settlement-worker`
- `workers/finance-reconcile`
- `workers/usage-receipt-dispatcher`
- `workers/provider-cost-reconciler`
- all nine live companions (they cannot earn remote evidence before deployment)

The following portions of otherwise-started deployables are also not complete:

- The eight generated billing HTTP operations, authorization, Aurora role probe, prepare/execute/
  finalize orchestration, and statements are not mounted in `finance-api`; it intentionally never
  reports ready in this branch.
- `central-schema-admin plan` is complete and credential-free. Mutating/verification/grant/backfill/
  repair commands validate their CLI and bundle but return stable `SecretUnavailable` rather than
  resolving a credential or touching AWS/Aurora. Re-entrant online execution and grant application
  therefore remain.
- The Stripe command edge's unknown-effect lookup currently searches PaymentIntents only. Customer,
  Checkout Session, Portal Session, and Refund lookup/search behavior still needs the exact generated
  lookup vocabulary and reconciliation policy.
- The webhook edge produces the accepted eight-event bounded envelope, but the landed generated Rust
  payment event contract represents only five events; it cannot yet be consumed byte-for-byte by
  `finance-ingest`.
- No workload descriptor was assigned to `central-finance` by the current workload registry. No
  remote/live Stripe, AWS, Aurora, queue, IAM, load, or soak evidence was attempted.
- Lambda memory, timeout, and reserved-concurrency values cannot be added to `[package.metadata.aex]`
  because the binding test-architecture schema has a closed thirteen-key vocabulary and rejects them.
  The deployment/infrastructure owner must provide the sanctioned manifest location. Direct-invoke
  Rust Lambdas also do not yet expose HTTP health routes; only the HTTP `finance-api` does.

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
- `aex_finance_aurora::store::{FinanceDbConfig, ConfigError}`
- `aex_finance_aurora::tx::{PostedReceipt, CommitProbe, CommitResolution, UnknownCommitError}`

Temporary cross-stream types, all carrying the required replacement comment:

- `aex_finance_aurora::wire_pending::CommitOutcomeUnknown` ->
  `aex_rds_data::transaction::CommitOutcomeUnknown`
- `aex_finance_aurora::wire_pending::RdsField` -> `aex_rds_data::row::FieldValue`
- `services/stripe-command-edge/src/wire_pending.ts::PaymentCommandEnvelope` ->
  `aex-payment-contracts::PaymentCommandEnvelope`
- `services/stripe-command-edge/src/wire_pending.ts::PaymentCommand` ->
  `aex-payment-contracts::PaymentCommand`
- `services/stripe-command-edge/src/wire_pending.ts::PaymentCommandFailure` ->
  `aex-payment-contracts::PaymentCommandResult`
- `services/stripe-webhook-edge/src/wire_pending.ts::ProviderEventEnvelope` ->
  `aex-payment-contracts::ProviderEventEnvelope`

## Changes needed from peers

1. Central identity must publish `aex_rds_data::transaction::CommitOutcomeUnknown` and an integer/
   string/null row enum at `aex_rds_data::row::FieldValue`; it must expose no `doubleValue` value
   accessor. Replace the two Aurora temporary types when that lands.
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
  changing it would violate the explicit quantum.
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
