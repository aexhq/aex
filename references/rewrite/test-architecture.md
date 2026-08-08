---
title: Test architecture as landed — ownership metadata, derived registry, flake scanner
description: What the test-architecture stream implemented on rw/testarch — the closed [package.metadata.aex] schema, the four policy documents, the derived test registry and its exact failure messages, the no-skip/no-retry scanner, the two harness packages, the nextest profiles, and every decision taken that is not already in the orchestrator conventions.
keywords:
  - testing
  - test registry
  - ownership metadata
  - flake policy
  - nextest
  - load harness
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-01
related:
  - references/rewrite/README.md
  - references/rewrite/delivery.md
---

# Test architecture as landed

Plans of record: `references/rust-native-rewrite-2026-07-31/plans/15-test-architecture.md` and `references/rust-native-rewrite-2026-07-31/plans/14-delivery-ci-infra.md`, in the parent workspace.

Branch `rw/testarch`. Nothing here authors a product test; it defines how a
package declares what it owes, turns those declarations into a checked registry,
and proves a green result contains no skip, no retry and no unexplained residue.

Everything below is executable. `cargo run -p aex-workspace-check` runs the
structural rules and the registry rules against the real tree and currently
reports:

```
aex-workspace-check: 133 member(s) and 139 package(s) satisfy every structural and registry rule
aex-workspace-check: 585 unearned-evidence row(s) recorded in the source-rewrite phase
```

---

## 1. What landed

| Artifact | Path |
| --- | --- |
| Closed external-seam registry, 34 rows | `release/policy/seams.toml` |
| Role→minimum-evidence profiles, closed value sets, per-lane TTLs | `release/policy/test-profiles.toml` |
| Local container substrate, pinned by resolved digest | `release/policy/test-images.toml` |
| Blocking prelaunch capacity gates and their owners | `release/policy/workload-registry.toml` |
| Ownership schema, parser, closed key set | `tools/aex-workspace-check/src/testmeta.rs` |
| Policy reader and its self-checks | `tools/aex-workspace-check/src/policy.rs` |
| Derived test registry, rules and emitter | `tools/aex-workspace-check/src/registry.rs` |
| No-skip/no-retry scanner and the receipt `inventory` block | `tools/aex-workspace-check/src/flake.rs` |
| Tree reader (manifests, npm, workloads, source scan) | `tools/aex-workspace-check/src/collect.rs` |
| Deliberate-failure fixtures | `tools/aex-workspace-check/tests/failure_fixtures.rs` |
| Generated properties over the checker | `tools/aex-workspace-check/tests/properties.rs` |
| Run identity, prefix, budget, TTL, cleanup ledger, canary, fault ports, image registry | `tests/support/aex-test-harness/` |
| Arrival process, driver, recorder, sampler, workload descriptor, gate contract | `tests/load/aex-load-harness/` |
| Tier profiles, descriptor JSON Schema, workload directory | `tests/load/profiles/`, `tests/load/schema/`, `tests/load/workloads/` |
| Six nextest profiles and the unit-lane default filter | `.config/nextest.toml` |
| Derived registry and the unearned-evidence ledger | `release/test-registry.json`, `release/unearned-evidence.json` |

Two new workspace members, both `publish = false` and dev-dependency only, both
outside `crates/` so Area 9's frozen 64-crate inventory is unchanged:
`tests/support/aex-test-harness` and `tests/load/aex-load-harness`. The member
count is 133.

---

## 2. The metadata schema, as landed

Exactly one table per manifest. Thirteen keys, closed: nine adopted verbatim
from the delivery plan (`owner`, `role`, `artifact`… see below) plus the four
this stream added (`artifact`, `deployable`, `targets`, `not_applicable`). A key
outside the set fails; a value outside a closed set fails.

```toml
[package.metadata.aex]
owner          = "regional-stores"           # closed set
role           = "adapter"                   # closed set
artifact       = "none"                      # closed set
deployable     = "regional-session-api"      # required for role deployable / runtime_image
live_suite     = "aex-live-regional-session-api"
layers         = ["unit", "integration"]     # unit | smoke | integration | e2e | user
concerns       = ["contract", "property", "fault", "security"]
seams          = ["aws.s3.conditional_put", "aws.kms.encryption_context"]
security_tier  = "authority"                 # closed set
risk           = ["concurrency"]             # closed set, drives fuzz/Miri/mutants
scenarios      = []

# Test target -> layer. Every [[test]] target of the package appears here, and
# every key here is a real [[test]] target. The library's own #[cfg(test)]
# module is always layer `unit` and needs no row.
[package.metadata.aex.targets]
requests    = "unit"
integration = "integration"

# Only where the type makes it structurally true. Free text, and the reason is
# checked: see section 4.
[package.metadata.aex.not_applicable]
targets = "awaiting the regional-stores stream; the cases land with the crate's own implementation"
```

npm manifests carry the identical object under a top-level `"aex"` key
(`packages/sdk`, `apps/site`, `apps/dashboard`, `apps/user-tests`,
`tools/eslint-plugin-aex`).

### 2.1 Closed value sets

They live once, in `release/policy/test-profiles.toml` under `[values]`, and are
embedded by both the checker and `aex-test-harness`, so the two cannot drift.

| Field | Values |
| --- | --- |
| `owner` | `contracts`, `central-identity`, `central-finance`, `regional-domains`, `regional-stores`, `regional-services`, `brain-core`, `providers`, `tools-mcp`, `hands`, `observations-usage`, `clients`, `infrastructure`, `delivery`, `test-architecture` |
| `role` | `contract`, `domain`, `application`, `adapter`, `composition`, `deployable`, `runtime_image`, `test_support`, `live_companion`, `tool`, `client`, `web_app`, `infra_module`, `bundle` |
| `artifact` | `none`, `lambda_zip`, `oci_image`, `oci_task`, `native_binary`, `npm_package`, `vercel_build_output`, `static_site`, `hands_image`, `terraform_module`, `bundle` |
| `layers` | `unit`, `smoke`, `integration`, `e2e`, `user` |
| `concerns` | `contract`, `property`, `concurrency`, `fault`, `security`, `performance`, `compatibility` |
| `security_tier` | `public_edge`, `authority`, `money`, `secret`, `guest`, `internal`, `diagnostic` |
| `risk` | `untrusted_input`, `unsafe_code`, `money`, `concurrency`, `sql`, `iam`, `generated_code`, `none` |
| `seams` | only ids present in `release/policy/seams.toml` (34 rows) |
| `not_applicable` keys | `live_suite`, `deployable`, `targets`, `smoke`, `e2e`, `integration`, `user`, `scenarios` |

### 2.2 One block per role, as applied

```toml
# crates/aex-session-domain — role = domain
[package.metadata.aex]
owner = "regional-domains"
role = "domain"
artifact = "none"
layers = ["unit"]
concerns = ["property"]
seams = []
security_tier = "authority"
risk = ["concurrency"]
scenarios = []

[package.metadata.aex.not_applicable]
targets = "awaiting the regional-domains stream; the cases land with the crate's own implementation"
live_suite = "pure domain crate; Area 9's registry declares no independent live seam"
```

```toml
# crates/aex-content-aws — role = adapter
[package.metadata.aex]
owner = "regional-stores"
role = "adapter"
artifact = "none"
live_suite = "aex-live-regional-session-api"
layers = ["unit", "integration"]
concerns = ["contract", "property", "fault", "security"]
seams = ["aws.s3.conditional_put", "aws.s3.multipart", "aws.s3.unversioned_delete", "aws.s3.presigned_expiry", "aws.s3.sse_kms", "aws.kms.encryption_context"]
security_tier = "authority"
risk = ["concurrency"]
scenarios = []

[package.metadata.aex.not_applicable]
targets = "awaiting the regional-stores stream; the cases land with the crate's own implementation"
```

```toml
# services/regional-session-api — role = deployable
[package.metadata.aex]
owner = "regional-services"
role = "deployable"
artifact = "lambda_zip"
deployable = "regional-session-api"
live_suite = "aex-live-regional-session-api"
layers = ["unit", "smoke", "e2e"]
concerns = ["contract", "fault", "security", "performance"]
seams = ["aws.dynamodb.transact_write", "aws.dynamodb.condition", "aws.s3.conditional_put", "aws.iam.denial", "aws.alb.framing"]
security_tier = "public_edge"
risk = ["iam", "untrusted_input"]
scenarios = []

[package.metadata.aex.not_applicable]
targets = "awaiting the regional-services stream; the cases land with the binary's own implementation"
```

```toml
# tests/live/aex-live-regional-session-api — role = live_companion
# `seams` is the union of every package that names this companion in live_suite,
# so a seam can never be declared by a crate and claimed by nobody.
[package.metadata.aex]
owner = "regional-services"
role = "live_companion"
artifact = "none"
deployable = "regional-session-api"
layers = ["smoke", "e2e"]
concerns = ["fault", "security", "performance"]
seams = ["aws.alb.framing", "aws.dynamodb.condition", "aws.dynamodb.streams", "aws.dynamodb.throughput", "aws.dynamodb.transact_write", "aws.dynamodb.ttl", "aws.iam.denial", "aws.kms.encryption_context", "aws.s3.conditional_put", "aws.s3.multipart", "aws.s3.presigned_expiry", "aws.s3.sse_kms", "aws.s3.unversioned_delete"]
security_tier = "public_edge"
risk = ["iam"]
scenarios = []

[package.metadata.aex.not_applicable]
targets = "awaiting the regional-services stream; live evidence cannot be earned before deployment (OD-07)"
```

```toml
# tools/aex-workspace-check — role = tool, with real targets rather than an excuse
[package.metadata.aex]
owner = "test-architecture"
role = "tool"
artifact = "none"
layers = ["unit"]
concerns = ["contract", "property"]
seams = []
security_tier = "diagnostic"
risk = ["none"]
scenarios = []

[package.metadata.aex.targets]
workspace = "unit"
properties = "unit"
failure_fixtures = "unit"

[package.metadata.aex.not_applicable]
live_suite = "workspace tool; it reads manifests and never reaches a deployed plane"
```

```json
// packages/sdk — role = client (npm)
"aex": {
  "owner": "clients", "role": "client", "artifact": "npm_package",
  "layers": ["unit", "user"], "concerns": ["contract", "compatibility"],
  "seams": ["npm.registry.publish"], "security_tier": "public_edge",
  "risk": ["generated_code"], "scenarios": [],
  "not_applicable": {
    "targets": "awaiting the clients stream; the SDK is regenerated over the new wire (OD-08)",
    "live_suite": "clean-install and packed-tarball evidence is package-owned; the SDK has no deployed endpoint of its own, and D-18's aex-live-sdk is outside the frozen member set"
  }
}
```

`test_support`, `composition`, `contract`, `application`, `runtime_image`,
`web_app` and the remaining `tool` blocks follow the same shape. The role
distribution across the 139 declared packages: 34 `live_companion`, 29
`adapter`, 28 `deployable`, 15 `domain`, 9 `application`, 7 `test_support`, 5
`contract`, 5 `tool`, 3 `client`, 2 `composition`, 1 `runtime_image`, 1
`web_app`.

---

## 3. Awaiting owner is not the same as silently omitted

This is the single mechanism that makes a skeleton workspace checkable. Every
`tests/live/*` package and almost every crate is a doc-comment stub with zero
test targets, which is the correct state before the product streams land — but
it must be *stated*, not inferred.

- **Awaiting owner.** The package declares `not_applicable.targets` with a
  reason that **contains its own `owner` value**. The registry records an
  `awaiting_owner` row in `release/unearned-evidence.json` and the package
  passes in the `source-rewrite` phase.
- **Silently omitted.** The package declares no targets and no reason. It fails
  `aex-empty-unit` immediately.
- **A reason that is not structural.** `"not yet written"` fails
  `aex-not-applicable-unjustified`, because it does not name the owner.
- **Both.** Declaring `not_applicable.targets` *and* targets fails
  `aex-not-applicable-unjustified`.
- **Candidate phase.** `--phase candidate` turns every unearned row, including
  every `awaiting_owner` row, into an `aex-unearned-evidence` failure. The
  excuse is a bookmark, never a permanent state.

Current ledger: 585 rows — 138 `awaiting_owner`, 295 `requires_live_seam`, 133
`requires_deployment`, 15 `pending_workload`, 4 `pending_authority`.

---

## 4. Exact failure messages

Every message uses the existing `Violation` shape, `[<rule>] <detail>`. The
registry rules map onto the delivery tool's existing exit code `10` (graph
verification) and the flake rules onto `41` (skip/retry/residue); this stream
introduces no new exit code.

### 4.1 Registry rules

| Rule | Message |
| --- | --- |
| `aex-metadata-missing` | ``​`crates/aex-foo` has no [package.metadata.aex] table; every member declares its test ownership`` (npm: ``has no "aex" table``) |
| `aex-metadata-unknown-key` | ``​`crates/aex-foo` declares unknown key `aex.tier`; the schema is closed`` |
| `aex-metadata-unknown-key` | ``​`crates/aex-foo` marks unknown field `property` not-applicable; excusable fields: live_suite, deployable, targets, smoke, e2e, integration, user, scenarios`` |
| `aex-metadata-bad-enum` | ``​`crates/aex-foo` declares role `helper`; permitted roles: contract, domain, …`` |
| `aex-metadata-bad-enum` | ``​`crates/aex-foo` declares `aex` as a string, not a table`` |
| `aex-role-profile-shortfall` | ``​`crates/aex-content-aws` is role `adapter` and must declare concern `fault`; declared: contract, property, security`` |
| `aex-role-profile-shortfall` | ``​`crates/aex-foo` is role `adapter` and must declare layer `integration`; declared: unit`` |
| `aex-not-applicable-unjustified` | ``​`services/regional-otlp` cannot mark `smoke` not-applicable; role `deployable` always has a started artifact`` |
| `aex-not-applicable-unjustified` | ``​`crates/aex-foo` marks `targets` not-applicable without naming its owner `regional-domains`; "not yet written" is not a structural reason`` |
| `aex-not-applicable-unjustified` | ``​`crates/aex-foo` marks `targets` not-applicable and then declares 2 of them`` |
| `aex-not-applicable-unjustified` | ``​`crates/aex-foo` marks `live_suite` not-applicable with no structural reason`` |
| `aex-empty-unit` | ``​`crates/aex-foo` target `properties` declares layer `unit` but collects 0 tests`` |
| `aex-empty-unit` | ``​`crates/aex-foo` declares no [package.metadata.aex.targets] row and no not_applicable.targets reason; an unwritten suite must name the stream that owes it`` |
| `aex-target-missing` | ``​`crates/aex-session-domain` declares concern `property` but has no [[test]] target mapped to layer `unit` (conventionally named `properties`)`` |
| `aex-target-unknown` | ``​`crates/aex-foo` maps target `properties` to a layer but declares no [[test]] target named `properties``` |
| `aex-target-unmapped` | ``​`crates/aex-foo` has [[test]] target `facade` mapped to no layer; every target's layer is a manifest fact`` |
| `aex-layer-uncovered` | ``​`crates/aex-foo` declares layer `integration` but maps no target to it and gives no not_applicable.integration reason`` |
| `aex-load-target-misplaced` | ``​`crates/aex-foo` declares target `load`, which the unit lane's default-filter does not exclude; D-11 puts every load and soak executor inside a live companion`` |
| `aex-orphan-companion` | ``​`tests/live/aex-live-ghost` names deployable `ghost`, which is not a workspace member`` |
| `aex-orphan-companion` | ``​`tests/live/aex-live-ghost` names no deployable and no not-applicable reason; a companion with no subject observes nothing`` |
| `aex-uncovered-deployable` | ``​`workers/usage-compute-worker` declares no live_suite and no not-applicable reason`` |
| `aex-unknown-live-suite` | ``​`crates/aex-foo` names live_suite `aex-live-ghost`, which is not a live companion package`` |
| `aex-live-target-underived` | ``​`tests/live/aex-live-foo` is named by no live_suite declaration; the live target set is derived from live_suite, never hand-listed`` |
| `aex-unknown-seam` | ``​`crates/aex-brain-hands` declares seam `aws.firecracker.boot`, which is not in release/policy/seams.toml`` |
| `aex-unclaimed-seam` | ``seam `aws.dynamodb.streams` is declared by `aex-session-dynamodb` but no live companion claims it`` |
| `aex-missing-evidence-class` | ``​`aex-usage-rating` owes evidence class `usage-rating-golden-invoice` (Area 9); no target declares concern `property``` |
| `aex-missing-evidence-class` | ``​`aex-usage-rating` owes evidence class `usage-rating-golden-invoice` (Area 9) but is not a declared package`` |
| `aex-test-support-in-production` | ``​`runtimes/brain-mux` links `aex-brain-test-support` as a normal dependency`` (transitive closure, not just the direct edge) |
| `aex-banned-feature` | ``​`runtimes/brain-mux` declares feature `chaos`; no package may carry a fault, test-hook, chaos, debug-endpoint or bypass feature`` |
| `aex-workload-unowned` | ``workload `brain-500-offered` names owner package `brain-mux-load`, which does not exist`` |
| `aex-workload-unowned` | ``workload `x` implements gate `Y`, which is not in release/policy/workload-registry.toml`` |
| `data-image-literal` | ``​`crates/aex-content-aws/tests/integration.rs` names a container image directly; call aex_test_harness::images so the digest pin is the only reference`` |
| `aex-unearned-evidence` | ``​`crates/aex-foo` still owes aex-empty-unit (awaiting the regional-domains stream); the candidate phase admits no unearned evidence`` |
| `aex-registry-stale` | ``release/test-registry.json differ(s) from what `aex-workspace-check registry build` produces; regenerate rather than hand-merging`` |

### 4.2 Flake rules

| Rule | Message |
| --- | --- |
| `flake-inventory-mismatch` | ``declared 412 cases, collected 409; missing: aex-session-domain::properties::journal_fold_determinism, …`` |
| `flake-skipped-test` | ``​`aex-finance-domain::properties::balanced` has ignored=true; a skipped test is a deleted test that still reports as coverage`` |
| `flake-skipped-test` | ``​`aex-finance-domain::properties::balanced` reported <skipped/>; a skipped test is a deleted test that still reports as coverage`` |
| `flake-skipped-test` | ``​`cargo test --doc` reported 3 ignored doctest(s); a skipped test is a deleted test that still reports as coverage`` |
| `flake-ignored-attribute` | ``crates/aex-foo/tests/properties.rs:88 uses #[ignore]; move the case to tests/live/ or delete it`` |
| `flake-self-skip` | ``crates/aex-foo/tests/integration.rs:24 reads env `AEX_PG_URL` directly; use aex_test_harness::required_env! so an absent prerequisite fails`` |
| `flake-empty-target` | ``​`aex-live-site::smoke` is a declared target and collected 0 cases`` |
| `flake-empty-selection` | ``filter `binary(smoke)` selected 0 of 118 cases in `aex-live-brain-mux``` |
| `flake-retry-configured` | ``profile `live` declares retries = 1; blocking lanes never retry to green`` |
| `flake-retry-configured` | ``the recorded environment sets NEXTEST_RETRIES=2; blocking lanes never retry to green`` |
| `flake-flaky-pass` | ``​`aex-live-regional-stream::seams::reconnect` passed on attempt 2; a flaky pass is a failing receipt`` |
| `flake-doctest-missing` | ``​`aex_wire` declares layer `unit` but no `cargo test --doc` result was supplied`` |
| `flake-quarantine-file` | ``​`.test-quarantine.json` exists; Q-FLAKE removes all release exemptions`` |
| `flake-first-failure-lost` | ``diagnostic rerun for receipt 7c1f does not carry the first failure; the original verdict may not be discarded`` |

### 4.3 Load-harness messages

| Rule | Message |
| --- | --- |
| `perf-workload-undeclared` | ``[perf-workload-undeclared] workload `brain-100-mixed` gate `LOAD-100-COMPLETE` declares kind `slo`; a prelaunch gate is a budget or a diagnostic, never a customer SLO`` |
| `perf-gate-unmet` | ``[perf-gate-unmet] gate LOAD-200-SAFE: rss_peak 1.94 GiB exceeds 0.85 x 2 GiB = 1.70 GiB (source: PERF-02)`` |
| unset budget | ``workload `brain-100-mixed` declares no budget_micro_usd; a live campaign with no spend ceiling cannot be stopped`` |
| unpinned image | ``image `postgres` (library/postgres:17.5-bookworm) has no digest in release/policy/test-images.toml; every testcontainers image is pinned by digest, not tag`` |
| absent prerequisite | ``required environment variable `AEX_PG_URL` is absent; a live prerequisite is a failure, never a skip`` |
| over-called port | ``scripted port `store` was called 2 time(s) but only 1 response(s) were programmed`` |
| unsupported counter | ``​`rss_bytes` cannot be sampled on windows; a load gate that asserts it must run on a platform that exposes it`` |

### 4.4 Deliberate-failure fixtures

`tools/aex-workspace-check/tests/failure_fixtures.rs` exhibits each defect and
asserts the exact message: a package with no declared evidence, an orphaned live
companion, an uncovered deployable, a skipped test, an ignored test (both from
the declared inventory and from source), a filtered-to-empty selection, a
retried test (both a flaky pass and a configured retry), an unknown metadata
key, a bare environment read, a target that collects nothing, and the
awaiting-owner / silently-omitted pair. A clean-lane fixture asserts the scanner
produces no violation and a zeroed inventory, so the fixtures prove the rules
fire *and* prove they do not fire spuriously.

---

## 5. The pinned local substrate

`release/policy/test-images.toml` is the only place a container image may be
named. Each entry carries the resolved digest, not a tag, and states what it
cannot prove — which is the reason its seams carry `requires_live = true`.

| Key | Repository | Digest |
| --- | --- | --- |
| `postgres` | `library/postgres:17.5-bookworm` | `sha256:fbcea1bd13b6a882cd6caa6b58db3ae5c102efe50ec625b3e2a5cbc50db5bfe4` |
| `dynamodb_local` | `amazon/dynamodb-local:2.6.1` | `sha256:1856c05cc66a0e49dc1099e483ad2851477eeebe2135250ac11a1d1227db54b1` |
| `minio` | `minio/minio:RELEASE.2025-04-22T22-12-26Z` | `sha256:a1ea29fa28355559ef137d71fc570e508a214ec84ff8083e39bc5428980b015e` |
| `moto` | `motoserver/moto:5.2.2` | `sha256:d8ae5edc2bf080e7e4c13f9bd4b29b53ac3b4427e92956318db3dbe23ec43eb7` |
| `toxiproxy` | `ghcr.io/shopify/toxiproxy:2.12.0` | `sha256:9378ed52a28bc50edc1350f936f518f31fa95f0d15917d6eb40b8e376d1a214e` |
| `stripe_mock` | `stripe/stripe-mock:v0.194.0` | `sha256:b535ce5548783b44ca0df7ecbe7f4773aee9f1b9caa6b012f0e0bc92844774b5` |

`aex_test_harness::images::reference(key)` returns
`<registry>/<repository>@<digest>` or `ImageError::Unpinned`. An entry with an
empty digest cannot produce a runnable reference at all, so an unpinned image is
a type-level impossibility rather than a review item. ClickHouse is absent:
Area 11 removes it, so `Q-DEPENDENCY`'s "real pinned ClickHouse" clause is void.

---

## 6. Lanes

`.config/nextest.toml` carries six profiles. `retries = 0` and
`fail-fast = false` in every one; every profile archives JUnit.

| Profile | `slow-timeout` | JUnit report-name | Used by |
| --- | --- | --- | --- |
| `default` | `30s`, terminate-after 4 | `aex-default` | local |
| `ci` | `60s`, terminate-after 4 | `aex-ci` | unit lane |
| `integration` | `120s`, terminate-after 4 | `aex-integration` | integration lane |
| `live` | `300s`, terminate-after 2 | `aex-live` | smoke and e2e lanes |
| `load` | `1800s`, terminate-after 1 | `aex-load` | load lane |
| `soak` | `3600s`, terminate-after 24 | `aex-soak` | soak lane |

`default` and `ci` carry `default-filter = 'not package(/^aex-live-/)'`. Every
lane passes `--no-tests=fail`.

The scanner is a real lane step:

```
cargo nextest list  -p <pkg>… --all-targets --message-format json --profile ci > list.json
cargo nextest run   -p <pkg>… --profile ci --no-tests=fail
cargo test --doc    -p <pkg>… > doctest.txt
cargo run -p aex-workspace-check -- flake scan \
  --list list.json --junit target/nextest/ci/junit.xml --doctest doctest.txt \
  --profile ci --package <pkg>…
```

which on the three packages this stream owns emits, and exits `0`:

```json
{ "declared": 191, "collected": 191, "skipped": 0, "ignored": 0,
  "filtered_at_runtime": 0, "retried": 0, "flaky": 0 }
```

---

## 7. The two harness packages

### `tests/support/aex-test-harness`

`TestRun` mints one identity per live, e2e, user, load or soak run — `tr_` plus
32 hex of a UUIDv7 — and everything else derives from it: the `aextest-<id>-`
resource prefix, the `TEST#<id>#` DynamoDB partition prefix, the `test/<id>/` S3
prefix, the five `aex:test-*` tags, and a `blake3(run_id ‖ logical_key)`
idempotency key so two runs never collide. The fifth tag is
`aex:test-synthetic`, the marker the janitor refuses to reclaim anything
without; it and the run-id shape, the lane TTLs, the residue grace and the
reclaimable resource table are all rows in `release/policy/test-profiles.toml`,
which both this crate and `aex-release-tool` embed.

`Budget::charge` returns `Allowed`, `SoftExceeded` or `Killed`; the counter
saturates, and the verdict never improves once a run is stopped. `CleanupLedger`
is the one cleanup ledger, and it reclaims rather than only recording:
`release` calls the installed `Reclaimer` and stamps `released_at` only when
the deletion succeeded, `reclaim_all` sweeps what remains in the policy's
dependency order, and `Drop` fails naming every leak — reporting to stderr and
a counter instead only while the thread is already unwinding, where panicking
again would abort the process and destroy the original failure. A ledger with
no reclaimer installed cannot release anything, so the default is a loud leak
rather than a silent success. The four `*-test-support` copies are renamed
`FixtureLedger`: they track product-internal fixture state with no discovery
route a sweep could use, and they call `report_residue` rather than each
deciding again what to do during a panic. `SecretCanary` has a redacted
`Debug`, and `scan_for_leaks` reports shape and position and never the value.
`ScriptedClock`, `ScriptedPort` and the Toxiproxy `Proxy`/`Toxic` descriptions
are the only fault surfaces; an over-called scripted port fails rather than
inventing an answer. `required_env!` panics with the variable's name.

### `tests/load/aex-load-harness`

`ArrivalSchedule::open`/`closed` are pure functions of `(mix, seed)` — a
`SplitMix64` written out so a campaign replays identically across dependency
bumps. `Driver::run` enforces
`offered == completed + typed_rejected + failed` and returns `DriverError::LostWork`
when it does not. `Recorder` collects exact samples and reports the eighteen
mandatory metrics; a campaign omitting one is not comparable and is rejected at
descriptor parse time. `ProcProbe` returns `SamplerError::Unsupported` off Linux
rather than a zeroed sample. `WorkloadDescriptor::parse` rejects `kind = "slo"`,
an unsourced blocking budget, an unnormalized mix, an incomplete arrival mode
and an unknown metric; `verify` rejects an unknown tier and, in the candidate
phase, an unset spend ceiling.

---

## 8. Decisions taken

| # | Decision | Rationale |
| --- | --- | --- |
| T-01 | The registry, flake and policy machinery lives in `tools/aex-workspace-check`, not in `tools/aex-release-tool` as plan 15 D-02 proposed | The delivery stream is implementing `aex-release-tool` concurrently and its files are off limits to this stream. `aex-workspace-check` already parses `cargo metadata`, already owns the `Violation` shape and is already the workspace's structural gate. **`TODO(cross-stream)` for delivery:** call `aex_workspace_check::registry` and `::flake` from `aex-release-tool test-registry` and `flake scan`, or re-export them; the semantics, rule ids and messages are fixed here and must not be re-implemented. |
| T-02 | The concern→target rule checks the **layer** and only names the conventional target in its message | Plan 15's message names `properties` exactly, but the plan's own filled example (`aex-content-aws` with targets `requests`, `codec`, `integration` and concern `property`) contradicts a filename requirement, and `Q-SELECTION` forbids a filename convention from deciding what a lane runs. `release/policy/test-profiles.toml` records the conventional name per concern so the message still tells a stream where to put the case. |
| T-03 | `not_applicable.targets` is the awaiting-owner marker, and its reason must contain the package's own `owner` value | This is the mechanical difference between "the stream that owes this is X" and "nobody wrote it". Without the owner check, `"not yet written"` would satisfy the rule, which is the exact failure mode plan 15 §13 item 15 forbids. |
| T-04 | A package that excuses `live_suite` records its `requires_live` seams as unearned instead of failing `aex-unclaimed-seam` | D-18 wants `aex-live-sdk` and `aex-live-aex-cli`, which are outside the frozen member set, and this stream owns exactly two new members. Recording the seam as an unearned row keeps the gap visible and closable without inventing a member another stream owns. |
| T-05 | `LIVE_TARGETS` stays in `tools/aex-workspace-check/src/inventory.rs` as the frozen **member** list; the **lane** target set is derived from `live_suite` and cross-checked against it | Deriving the member list from metadata is circular — a package must exist to declare metadata. D-03's intent (no hand-maintained lane allowlist) is met by the derivation plus `aex-live-target-underived`; the frozen list keeps "a directory exists that nobody declared" a failure. Today: 31 derived, 3 awaiting their subject package, 34 total. |
| T-06 | The unit lane's `default-filter` is `not package(/^aex-live-/)`, not D-22's `package(/^aex-load-/)` addition | `aex-load-harness` is a package, so that pattern excludes the harness's own unit tests from every default run — an invisible skip. Load executors live in live companions (D-11), so the live exclusion already covers them, and `aex-load-target-misplaced` fails any package that declares one elsewhere. nextest also rejects `binary(load)` outright while no such binary exists. |
| T-07 | `release/test-registry.json` and `release/unearned-evidence.json` are committed and regenerated on conflict, exactly like `Cargo.lock` | An uncommitted registry makes `aex-registry-stale` dead code. The conflict cost is real and is paid with one command: `cargo run -p aex-workspace-check -- registry build`. |
| T-08 | `registry build` accepts an **optional** `--list`; the committed document is the declaration-only form | Building the committed registry must not require compiling every test binary in the workspace. Collected counts are a lane input; with no list the registry records `nextest_list: absent` and adds a `pending_authority` row, so the absence is visible rather than assumed. |
| T-09 | Rules whose authority file does not exist yet are recorded as `pending_authority` rows, never evaluated vacuously | "No route registry exists" and "every route has an owner" must not look the same in a report. Four rows today: the route registry, `release/scenario-ownership.toml`, the central migration bundle, and a nextest listing. |
| T-10 | `scenarios` is declared empty everywhere until `release/scenario-ownership.toml` exists | Declaring scenario ids with no owner file would make every one of them an orphan. The requirement is recorded as a pending authority instead. |
| T-11 | The closed value sets live in `release/policy/test-profiles.toml` and are embedded by both the checker and `aex-test-harness` | Two hand-maintained copies of a closed set is exactly the drift the schema exists to prevent. |
| T-12 | `is_test_support_crate` now matches `*-test-support`, `aex-test-harness`, `aex-load-harness` and every `aex-live-*`; `test-support-is-dev-only` exempts test-only *dependents* | Test-only code composes with test-only code — the load harness takes the test harness as a normal dependency, as every companion will. What must never happen is a production package reaching any of them, and the registry additionally checks the whole normal-dependency **closure**, not only the direct edge. |
| T-13 | The site package declares `deployable = "site"` and `live_suite = "aex-live-site"` | The documentation application *is* the site. The clients stream has since retired the old docs directory in favour of `apps/site`, which is where the declaration now lives. |
| T-14 | The four companions whose subject does not exist — dashboard, model catalog, the two Stripe edges — excuse `deployable` with the stream that lands it | Three of them would otherwise fail `aex-orphan-companion` for a true but not-yet-actionable reason. The excuse puts them in the ledger under the stream that can close it. |
| T-15 | Container digests were resolved by anonymous registry manifest queries, not by pulling | The §8a amendment requires digest pinning; no credential is involved in an anonymous `HEAD /v2/<repo>/manifests/<tag>`, and nothing was pulled or run. |
| T-16 | `tests/load/profiles` declares only `t2` and `t3` | They are the two tiers Area 10 rows name explicitly (100 peak active, 500 offered). Inventing further tiers would create thresholds nobody can defend when a campaign misses them. |
| T-17 | Scanner needles are assembled with `concat!` so the scanner never reports its own source | The alternative — exempting the file that defines the rule — is the quarantine `Q-FLAKE` removes. |
| T-18 | `tools/aex-workspace-check/tests/workspace.rs` reads `CARGO` through `aex_test_harness::required_env!` | The rule that forbids bare environment reads in test targets applies to the package that defines it first. |
| T-19 | The Hands shape arity is **five** (`512mb`, `1gb`, `2gb`, `4gb`, `8gb`), recorded as evidence class `hands-compute-shape-golden-table` owned by `aex-runtime-control` | The §8a amendment settles plan 15's §15 gap between plan 10's five and plan 12's six. The evidence class now resolves, so `aex-missing-evidence-class` no longer blocks on it. |

---

## 9. Known gaps

| Gap | Blocked on | Handling |
| --- | --- | --- |
| No live receipt of any kind can be earned (`OD-07`) | owner cut-readiness review, then dev credentials | 428 rows in `release/unearned-evidence.json` under `requires_deployment` and `requires_live_seam`, each naming its stream |
| `aex-live-sdk` and `aex-live-aex-cli` (D-18) do not exist | orchestrator ruling on the member set | `packages/sdk` and `tools/aex-cli` excuse `live_suite`; their `npm.registry.publish` seam is an unearned row |
| The dashboard application and the two TypeScript Stripe edges have no package yet | clients and central-finance streams | their companions excuse `deployable`; the derived live target set is 31 of the frozen 34 |
| A non-package artifact metadata file under `release/` is not authored | delivery stream owns `release/units.toml` and the artifact tree | the model catalog's companion excuses `deployable`; the file is a delivery input, not this stream's |
| Provider spend budgets have no numeric values | real BYOK credentials and a synthetic-account spend cap | `budget_micro_usd` is optional in `source_rewrite` and blocking in `candidate` |
| Terraform module metadata (`infra/modules/*/aex.toml`) is unexercised | `infra/modules/` is empty | the schema is defined; the reader is added with the first module |
| No workload descriptor exists yet | every load owner | 15 `pending_workload` rows, one per declared capacity gate |
| Coverage tooling is unselected | no blocking requirement — coverage is diagnostic | deferred |

---

## 10. What every product stream must satisfy

*Quote this verbatim into every implementation brief.*

1. Declare `[package.metadata.aex]` in every Cargo manifest you own (`"aex"`
   object for npm), using the closed value sets in `release/policy/test-profiles.toml`.
   A missing block fails `aex-metadata-missing`; an unknown key fails
   `aex-metadata-unknown-key`.
2. Declare at least your role's layers and concerns from
   `release/policy/test-profiles.toml`. More is fine, fewer fails
   `aex-role-profile-shortfall`.
3. Map every `[[test]]` target to a layer in `[package.metadata.aex.targets]`,
   and declare no target key that is not a real `[[test]]` target. The library's
   own `#[cfg(test)]` module is always layer `unit` and needs no row.
4. If your tests are not written yet, say so: `not_applicable.targets` with a
   reason that names your `owner`. Delete that line in the same change that
   lands your first target. "not yet written" is rejected.
5. Declare every external seam you touch by id from `release/policy/seams.toml`.
   Add a missing seam there in the same change with `proves_locally` and
   `requires_live`. Point `live_suite` at the companion that claims your
   `requires_live` seams, or excuse `live_suite` with a structural reason.
6. Never write a container image name, tag or digest in your crate. Call
   `aex_test_harness::images::reference("postgres")`. Gate every
   testcontainers-backed target behind `required-features = ["integration-engines"]`.
7. Use `aex_test_harness` for run identity, resource prefix, budget, TTL,
   cleanup ledger and secret canary. Do not define a second one of any of them.
   Record every created live resource in the ledger **before** the create call
   returns and release it on teardown.
8. No `#[ignore]`, no `#[cfg_attr(…, ignore)]`, no bare `std::env::var` or
   `option_env!` in a test target — use `aex_test_harness::required_env!` — no
   empty target, no retry-to-green, no quarantine or expected-failure file.
9. Never add a feature named `fault`, `fault-injection`, `test-hooks`, `chaos`,
   `debug-endpoints` or `bypass-*`, and never expose a test-only route. A
   test-support or live-companion package may never be a normal dependency of a
   production package, transitively.
10. A live companion imports only public and generated contracts plus test
    support, never an implementation-private module of its deployable. It fails
    loudly on an absent descriptor and never self-skips.
11. Put your load and soak executors in a `[[test]] name = "load"` target inside
    your live companion, driven by `aex-load-harness`. Besides that one shared
    package, `tests/load/` holds descriptors, tier profiles and fixtures only —
    add no member there, and add no member anywhere without the orchestrator.
12. Author `tests/load/workloads/<owner>/<id>.toml` for every gate
    `release/policy/workload-registry.toml` assigns you, with `tier`, `arrival`,
    at least one gate and the full eighteen-metric `report.metrics` list.
    `kind = "slo"` is rejected; use `kind = "budget"` with a `source` naming its
    Area 10 or `PERF-*` row.
13. Every durable command carries the eight-row unknown-outcome matrix, asserted
    on durable facts. "Eventually returned 200" is not a recovery test.
14. Apply Loom only to the six named kernels; anywhere else needs
    `risk = ["concurrency"]` and a named kernel in your plan. Adding
    `#![allow(unsafe_code)]` requires `risk = ["unsafe_code"]` in the same
    change.
15. Run `cargo run -p aex-workspace-check` before you finish. If you changed a
    manifest, also run `cargo run -p aex-workspace-check -- registry build` and
    commit the regenerated `release/test-registry.json` and
    `release/unearned-evidence.json`. Resolve a conflict in either by
    regenerating, never by hand-merging — the same rule as `Cargo.lock`.
