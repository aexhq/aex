---
title: Delivery stream handoff — release tool, CI lanes, Terraform, evidence
description: What the delivery stream implemented on rw/delivery, what it deliberately left undone, what every other stream must declare for graph verify to pass, and every decision taken that is not already in the orchestrator conventions.
keywords:
  - delivery
  - release
  - ci
  - terraform
  - evidence
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-01
related:
  - references/rewrite/README.md
  - references/rewrite/test-architecture.md
---

# Delivery stream handoff

Plans of record: `references/rust-native-rewrite-2026-07-31/plans/14-delivery-ci-infra.md` and `references/rust-native-rewrite-2026-07-31/plans/15-test-architecture.md`, in the parent workspace.

Branch `rw/delivery`. Nothing is deployed, published, credentialed or applied.
Every gate below was run locally and passes; the one intentional exception is
`graph verify` against the real repository, which is red by design and is the
first item under "What every stream owes me".

## 1. What was implemented

### `tools/aex-release-tool/`

One library plus a thin binary shell. 259 tests, all passing, no `#[ignore]`,
no self-skip, no empty suite.

| Module | What it owns |
| --- | --- |
| `canon` | RFC 8785 JCS, the one workspace canonicalizer. Rejects floating-point numbers outright rather than serializing them. UTF-16 member ordering, so an astral-plane key sorts where the spec says. |
| `error` | The 21-value exit-code contract and the `[rule] detail` violation shape. Checks accumulate; nothing exits `0` on a warning; there is no `--force`. |
| `meta` | The delivery-side `AexMeta` parser: thirteen closed keys, eight closed value sets. `aex-workspace-check` owns the registry rules over the same block; `test_registry` below asserts the two authorities agree. |
| `graph` | Namespaced `NodeId`, CSR forward and reverse adjacency, per-class cycle detection, four merged authorities, prefix-safe path classification, selection with recorded reasons, shadow mode, LPT matrix partitioning. |
| `pack` | Byte-stable ZIP, tar and gzip writers. Every header field is written explicitly; only the DEFLATE stream is borrowed. |
| `artifact` | Recipes, deterministic packaging, the `aex.artifact-envelope.v1` type, envelope verification, publication destinations. |
| `manifest` | `aex.composition-manifest.v1`, the strict environment scan, the default stage order, rollback candidates. |
| `evidence` | `aex.evidence-receipt.v1`, the no-skip counters, JUnit element counting, freshness classes, lane aggregation. |
| `verification` | `aex.verification-statement.v1` and its binding to a manifest. |
| `admit` | The nine numbered promotion rules, each with its own exit code. |
| `ledger` | The deployment state machine, the monotonic fence, desired/actual convergence. |
| `migration` | Bundle packaging, header parsing, below-head insert and edit rejection. |
| `private_path` | The public classifier the private repository invokes, with its own glob matcher. |
| `policy` | Terraform source policy, Dockerfile COPY-only policy, workflow structural gates, Terraform plan policy. |
| `schemas` | The five release JSON Schemas, embedded. |
| `selftest` | Determinism and monotonicity probed against the real graph. |
| `test_registry` | Reads `release/test-registry.json` and `release/unearned-evidence.json` through `aex-workspace-check`'s own types. Derives nothing; asserts the delivery graph and the registry agree on the live set. |

Subcommands landed: `graph build|verify|select|explain|diff|matrix`,
`artifact recipes|plan|package|verify|publish-plan`,
`manifest digest|validate|order|rollback-candidates`,
`evidence verify|require|aggregate`, `verification verify`, `admit`,
`plan summarize|policy`, `ledger append|list|verify`, `private-path check`,
`migration bundle|verify`, `policy terraform|workflows|dockerfile`, `schema`,
`selftest`.

### `api/schemas/release/`

Five schemas, authored here per D14-05 so the contract generator needs no
special case: `artifact-envelope.json`, `composition-manifest.json`,
`verification-statement.json`, `environment-binding.json`,
`evidence-receipt.json`. The contracts stream had not created that directory,
so there is no conflict to record.

### `release/`

`units.toml` (29 deployables), `scenario-ownership.toml` (20 scenarios),
`path-map.toml` (47 rules, zero orphans against the current tree),
`release/policy/artifact-policy.toml`, `release/policy/freshness.toml`,
`release/policy/terraform-policy.toml`, `release/policy/rollout-policy.toml`,
`release/policy/private-path-policy.json`.

### `.github/workflows/`

The old `ci.yml` and `live-user-tests.yml` are deleted, not aliased. Eleven
files: four lane classes (`pr`, `main`, `assurance`, `release`) and seven
reusable workflows (`_route`, `_rust-lane`, `_node-lane`, `_terraform-lane`,
`_build-artifacts`, `_receipts`, `_release-engine`). `.github/dependabot.yml`
gains cargo and terraform ecosystems.

### `infra/`

22 modules and 7 example roots, each with `main.tf`, `variables.tf`,
`outputs.tf`, `versions.tf`, `README.md`, `aex.toml` and
`tests/<name>.tftest.hcl` over `mock_provider`. 225 `run` blocks, zero
failures. Providers: `hashicorp/aws ~> 6.0` resolving 6.57.1,
`vercel/vercel ~> 5.0` resolving 5.7.1. No account id, ARN or real domain
appears in any `.tf`.

## 2. Gate output

```
cargo fmt --all                                              clean
cargo clippy -p aex-release-tool --all-targets -- -D warnings clean
cargo nextest run -p aex-release-tool                         259 passed, 0 skipped
cargo check --workspace --all-targets                         clean
cargo run -p aex-workspace-check                              133 members, 139 packages, every rule
terraform fmt -check -recursive infra/                        clean
terraform init -backend=false && validate && test             29/29 directories pass
```

`graph verify` against the real repository exits `10` with 32 violations, all
of them work another stream owes. That is the designed state, not a regression;
see §3.

## 3. What every stream owes me

`graph verify` is the gate. It is red today and names exactly who owes what.

### 3.1 `[package.metadata.aex]` — landed

All 139 manifests carry ownership metadata after the test-architecture stream
merged. `graph verify` reads real data and no longer reports
`aex-metadata-missing`. Four live companions still omit the `deployable`
back-reference their role requires: `aex-live-dashboard`,
`aex-live-model-catalog`, `aex-live-stripe-command-edge` and
`aex-live-stripe-webhook-edge`. They resolve once the clients and finance
streams land the packages those companions are about.

### 3.2 Lambda and Fargate resource shapes — 25 violations

`release/units.toml` carries no `[unit.lambda]` or `[unit.fargate]` block for
25 of the 29 deployables, because the accepted design declares isolation
requirements for them and no numbers. Inventing values here would have turned a
missing decision into a shipped one. Each owning stream fills its own rows:

- **central-identity/control**: `central-identity-api`, `central-authz`, `central-control-api`, `central-control-worker`, `central-schema-admin` (Fargate task shape).
- **central-finance**: `finance-api`, `finance-ingest`, `finance-settlement-worker`, `finance-reconcile`, `provider-cost-reconciler`.
- **regional-services**: `regional-session-api`, `regional-secret-api`, `regional-observation-api`, `regional-otlp`, `session-operation-worker`.
- **regional-stores**: `content-lifecycle-worker`, `regional-secret-key-admin` (Fargate task shape).
- **hands**: `runtime-control-worker`.
- **observations-usage**: `observation-reconciler`, `observation-export-launcher`, `observation-export-task` (Fargate task shape), `usage-storage-worker`, `usage-compute-worker`, `usage-transfer-worker`, `usage-receipt-dispatcher`.

`memory_mb` must be in 128..=10240 and `timeout_s` in 1..=900; a zero is
rejected as a placeholder. `brain-mux` and `regional-stream` already carry the
two shapes the accepted design pins.

### 3.3 Three live companions nobody claims — 3 violations

`aex-live-dashboard`, `aex-live-stripe-command-edge` and
`aex-live-stripe-webhook-edge` exist as workspace members and no package or
unit names them as a `live_suite`. They resolve once the clients stream lands
`apps/dashboard` and the finance stream lands the two TypeScript edges. Until
then the graph records the gap rather than hiding it.

### 3.4 Registries and interfaces

1. **A row in `release/units.toml`** for every deployable, and
   **a row in `release/scenario-ownership.toml`** observing it. A deployable no
   scenario observes fails verification.
2. **A declarative alarm spec id** (`alarm_spec`) and a documented
   graceful-shutdown contract per deployable.
3. **`api/generated/registries/routes.json`** with a `scenarios` array per
   `operationId`. When that file exists, `graph verify` requires every public
   route to name a declared scenario.
4. **`migrations/central/*.sql`** with an `-- aex-migration: tx= destructive= phase=`
   header on every file, `grants.toml` for privileges, and `bundle.lock.json`
   produced by `aex-release-tool migration bundle`. A `GRANT` inside a
   migration body is rejected; a non-transactional migration without a
   `.repair.sql` sibling is rejected.
5. **`release/policy/seams.toml`**, **`test-profiles.toml`**,
   **`test-images.toml`** and **`workload-registry.toml`** are the
   test-architecture stream's content inside my directory. I do not author or
   duplicate them; `aex-workspace-check` validates against them, and the
   release tool consumes their outputs.

### 3.5 `TODO(cross-stream)` raised by this stream

- **Plan 07 (brain)**: health endpoints are `/internal/healthz` and
  `/internal/readyz`. `graph verify` rejects any other value. `brain-mux`
  `desired_count` is pinned to 1 in `release/units.toml` and in the
  `ecs-service` module default.
- **`tools/aex-workspace-check`**: `inventory.rs` still holds a hand-written
  `LIVE_TARGETS`. The amendment says that list is derived from `live_suite`
  metadata. `aex-release-tool graph build` already derives it (29 targets
  today); the hand list in the scaffolding must be replaced by whoever owns
  that crate. I did not edit it.
- **`tools/aex-workspace-check`**: `MEMBER_ROOTS` needs `tests/support` and
  `tests/load` once the test-architecture stream lands `aex-test-harness` and
  `aex-load-harness`.
- **Plan 03 (finance)**: the `schema-head.json` the finance stream owes under
  `release/` is defined by `aex_release_tool::migration::SchemaHead`. Consume that shape rather than
  defining a second head file. `adminImageDigest` is filled by the build lane
  after the `central-schema-admin` image is packaged.
- **Plan 05 (regional stores)**: `regional-tables.json` must carry a
  `physicalNameTemplate` per table, an `iamActions` list per role, and the
  immutable keystore physical name. Physical naming elsewhere is
  `aex-{plane}-{region}-{logical}`.
- **The three `localhost` image tags** are duplicated in
  `infra/examples/localhost/compose.yaml` and in plan 15 §11. Once
  `release/policy/test-images.toml` exists, a cross-file check must assert the
  two agree.

## 4. What was deliberately left undone

1. **OCI and rootfs packaging.** `artifact package --form oci|rootfs` returns a
   typed refusal. An OCI image is assembled over a digest-pinned base whose
   blobs come from a registry, and a rootfs comes from the Hands image build;
   neither is producible offline. `artifact plan` still prints the exact build
   invocation for both, and the Dockerfile scanner enforces COPY-only.
2. **`artifact describe`.** The envelope type, its schema and its verification
   are complete; the subcommand that assembles one from a live build is not,
   because every field it would fill (`runId`, `builderId`, SBOM digest,
   attestation bundle) comes from a CI run that cannot happen yet. Envelopes
   are constructed in tests from the same types.
3. **`evidence new`/`attach` and `verification new` as subcommands.** The
   library functions exist and are tested; the CLI wrappers are not wired,
   because their inputs are JUnit and readbacks from runs that do not exist.
4. **The DynamoDB ledger store.** `LedgerStore` is a trait; the JSONL mirror is
   the only implementation. Fence and transition semantics are identical, so
   the DynamoDB adapter is a store, not a rewrite.
5. **`manifest new`/`diff` and `plan bind`.** `with_unit` and `order_for` carry
   the logic; the subcommands need a release object store to read the previous
   manifest from.
6. **`test-registry`, `flake scan`, `janitor` and `workload verify`.** By
   orchestrator ruling these live in `aex-workspace-check`, not here.
   `aex-release-tool` depends on that crate and consumes
   `release/test-registry.json` and `release/unearned-evidence.json` through
   its own types (`src/test_registry.rs`). Nothing is re-implemented; the rule
   ids and messages in `aex-workspace-check` are the authority. `janitor sweep`
   remains unimplemented in either crate — it needs a deployed plane and a
   dedicated identity.
7. **Live publication.** Every publish step in `main.yml` and every apply step
   in `_release-engine.yml` refuses with a stated reason and a classified exit
   code rather than pretending. The lanes are wired end to end; the buckets,
   repositories, roles and GitHub Environments are not.

## 5. Decisions taken

| ID | Decision | Why |
| --- | --- | --- |
| D-1 | The workflow structural gate is a Rust subcommand (`policy workflows`), not `scripts/validate/ci-*.test.ts` | One tool, one parser, no Node prerequisite for a Rust-only change — the same reasoning as OD-01. The gate now runs in the same binary as `graph verify`, so a lane cannot pass one and skip the other. |
| D-2 | `flate2` is added to `[workspace.dependencies]` | The only new dependency. `aex-release-tool` writes ZIP, tar and gzip itself because archive metadata must be byte-stable; borrowing just the DEFLATE stream is the smallest thing that achieves it. |
| D-3 | The JSON Schemas are stricter than the serde types, and the asymmetry is tested rather than removed | The schemas narrow several string fields to closed value sets that serde carries as `String`. `tools/aex-release-tool/tests/schemas.rs` asserts parity on structure — unknown members, missing members, wrong types — and asserts the narrowing separately. Making serde enums would force every unknown future value to be a parse failure at a layer that should report a violation instead. |
| D-4 | The `correctness-test` deny globs are `**/tests/**`, `**/*.test.*`, `**/*_test.*`, `**/*.spec.*`, `**/*_spec.*`, `**/*.tftest.hcl` rather than plan 14's `**/*test*` | `**/*test*` denies a data file such as a `latest-prices.json` under `business-data/`, because "latest" contains "test". The narrower set catches every correctness test without denying a data file for a substring. |
| D-5 | `graph verify` runs the registry-reference checks before building the graph | A `units.toml` row naming a package that does not exist produces a dangling edge and stops construction. Checking first means the report names the row a human has to fix rather than only that some edge pointed at nothing. |
| D-6 | An unowned path both widens the run to repo-wide and fails verification | Carried from D14-06. The tests assert both halves, because either alone is a failure mode: widening silently hides the gap, failing alone leaves a red build running a narrow selection. |
| D-7 | Cycle detection is per edge class, not over the union | A Cargo dev-dependency that points back at its dependent is legal and common. Detecting over the union would reject the workspace's own test wiring. |
| D-8 | `release/units.toml` holds only the 29 Rust deployables | `dashboard`, `site`, `aex-cli`, `@aexhq/sdk`, the two Stripe edges, the two catalogues and the three bundles are also artifacts, and their packages are not workspace members yet. Listing them would produce violations about packages nobody has created, drowning the violations about work that is actually owed. |
| D-9 | Publishing is not deploying, for the purpose of the permission-overlap gate | The gate fires when a job holds both publish scopes and applies a plan or holds a GitHub Environment. Uploading bytes to an immutable object store is publication; treating it as deployment would forbid the one job that must build and upload. |
| D-10 | The workflow linter reads both `on:` and `true:` | A YAML 1.1 reader resolves a bare `on:` key to the boolean `true`. Looking under both spellings keeps quoting the key a style choice rather than a way past the gate. |
| D-11 | `.terraform/`, `*.tfstate` and `*.tfstate.*` are added to `.gitignore`; `.terraform.lock.hcl` is not | `terraform init` drops provider binaries into the worktree. The lock file is deliberately tracked: the resolved provider versions are part of what a plan means. |
| D-12 | The `assurance` and `release` lanes exit with a classified code rather than a green no-op where their subject does not exist | A scheduled suite that reports success having checked nothing is worse than one that is red for a stated reason. `admit` then refuses on a missing receipt (exit 40) instead of accepting silence as evidence. |
| D-13 | `github-oidc-role` pins the workflow path through `job_workflow_ref`, not `sub` | GitHub carries the workflow path in `job_workflow_ref` unless subject customization is configured. Plan 14 §8.1 says `sub`; the module pins repo and ref via `sub` and the exact workflow path via `job_workflow_ref`, with no wildcard in either. Documented in that module's README. |
| D-14 | Example roots carry an `aex.toml` with `role = "composition"` | Fail-closed rule 1 requires every Terraform root to be classified. Without the sidecar `graph verify` would exit `10` on the examples this stream shipped. |
| D-15 | `aex-release-tool` depends on `aex-workspace-check` and consumes the derived registry rather than deriving a second one | Orchestrator ruling. Two parsers for one metadata block is exactly the drift both crates exist to prevent. `graph verify` adds one check neither authority can do alone: the live-target set the delivery graph derives must equal the one the registry derives, so a disagreement is a failure rather than a silent divergence. |
| D-16 | A missing required receipt that appears in `release/unearned-evidence.json` is refused with the owning stream named | Admission refuses either way, but a recorded, owned gap and a hole nobody noticed are different facts, and an operator reading exit 40 should not have to work out which one they have. |

## 6. Known gaps

1. **No `arch-qualification` evidence can be earned.** Nothing is deployed, so
   `aarch64` artifacts have no live receipt and `admit --plane prd` refuses them
   by design.
2. **No registry, no credentials.** ECR repositories, the artifact bucket, the
   npm `npm-release` environment and the OIDC roles do not exist. Publish and
   apply steps refuse rather than no-op.
3. **`hands-image` reproducibility** needs a date- or digest-pinned package
   source the Hands stream has not selected. Without one, two rootfs builds
   differ and the `determinism` receipt cannot pass.
4. **Vercel has no OIDC path** for CLI deployment. The scoped project token is
   the one long-lived credential in the design and needs a rotation owner.
5. **Attestation visibility** for a private repository requires Enterprise
   Cloud. The public `aex` repository is fine.
6. **The `cold-rebuild` suite's cost is unmeasured.** Rebuilding 29 artifacts
   cache-free weekly may exceed a sensible Actions budget. The fallback is a
   rotating subset with full coverage over four weeks, which weakens the
   guarantee and must be a recorded decision rather than a silent change.
7. **Restore-drill and quota-headroom receipts** need real AWS state.
8. **Predicate-only private-path deny rules** (`tf-file-declaring-resource-outside-roots`,
   `declares-policy-or-rate-logic`) are declared in the policy and not
   implemented; they are content predicates rather than path globs. The corpus
   test skips them explicitly rather than counting them as covered.
