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
last_verified: 2026-08-03
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
`artifact recipes|plan|package|describe|verify|publish-plan`,
`manifest new|diff|digest|validate|order|rollback-candidates`,
`evidence new|attach|verify|require|aggregate`,
`verification new|verify`, `admit`,
`plan summarize|policy`, `ledger append|list|verify`, `private-path check`,
`migration bundle|verify`, `policy terraform|workflows|dockerfile`, `schema`,
`selftest`.

`describe` is in `src/describe.rs` and assembles an envelope from a build that
happened on the machine running it. Every field the tree establishes — artifact
digest and size, toolchain read from `rustc -vV`, the exact argv, the input
closure taken from the graph, the lockfile — is read. Every field only a
workflow run can establish is set to `unearned` (or `0`, or `false`) and named
in a ledger the command writes beside the envelope. The result is refused by
`artifact verify` by construction, because a local path is not an immutable
location and nothing attested the bytes.

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

The old `ci.yml` and `live-user-tests.yml` are deleted, not aliased. Twelve
files: four lane classes (`pr`, `main`, `assurance`, `release`) and eight
reusable workflows (`_route`, `_rust-lane`, `_node-lane`, `_terraform-lane`,
`_scenario-lane`, `_build-artifacts`, `_receipts`, `_release-engine`).
`.github/dependabot.yml` gains cargo and terraform ecosystems.

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
cargo nextest run -p aex-release-tool                         316 passed, 0 skipped
cargo check --workspace --all-targets                         clean
cargo run -p aex-workspace-check                              133 members, 140 packages, every rule
terraform fmt -check -recursive infra/                        clean
terraform init -backend=false && validate && test             29/29 directories pass
```

The 2026-08-02 count of 148 graph violations is superseded by the actual-mount
and runnable-scenario correction below. The current authorities deliberately
imply 176 delivery violations while no scenario has an executable claim: 20
`scenario-runnable-missing`, 51 `aex-route-unserved`, and 105
`aex-route-uncovered` scenario references across the 95 actually served routes.
Malformed, cross-plane, and planned/actual owner disagreements are all zero.
That red state names missing work; it is not a release receipt.

## 3. What every stream owes me

`graph verify` is the gate. It is red today and names exactly who owes what.

### 3.1 `[package.metadata.aex]` — landed

All manifests carry ownership metadata. `aex-live-model-catalog` names
`brain-mux` as its deployable: the catalogue is loaded and enforced inside that
runtime rather than shipped as a standalone service. The former reference to a
nonexistent `release/artifact-metadata.toml` is removed, so the live companion
has a real started-artifact subject and `graph verify` no longer reports
`aex-metadata-missing-deployable`.

### 3.2 Lambda and Fargate resource shapes — landed

Every row that runs on Lambda or Fargate now carries its shape. The eight that
were still empty (the finance family, `runtime-control-worker` and
`central-schema-admin`) are derived from the shape a peer doing the same kind of
work already uses, and each row says which peer and why. `memory_mb` must be in
128..=10240 and `timeout_s` in 1..=900; a zero is rejected as a placeholder.

### 3.3 The two Stripe edges — landed

`services/stripe-{command,webhook}-edge` are npm packages under a Cargo member
root, so no `workspaces` glob reaches them. `aex-workspace-check` named them
explicitly and the delivery graph did not, which is why the two authorities
derived different live-target sets and `graph verify` reported
`live-target-disagreement`. `NPM_ROOTS` and `NPM_EXPLICIT` are now public on
`aex_workspace_check::collect` and the release tool reads that one list.
`GraphInputs::package_node` resolves a unit's owning package through the Cargo
and npm namespaces, so `release/units.toml` carries `ts-lambda` rows for both
edges and the path map routes their directories to their real npm nodes.

### 3.4 Registries and interfaces

1. **A row in `release/units.toml`** for every deployable, and
   **a row in `release/scenario-ownership.toml`** observing it. A deployable no
   scenario observes fails verification.
2. **A declarative alarm spec id** (`alarm_spec`) and a documented
   graceful-shutdown contract per deployable.
3. **`api/generated/registries/routes.json`** with a `scenarios` array per
   `operationId`. When that file exists, `graph verify` requires every public
   route to name a declared scenario.

   **Corrected 2026-08-03.** `routes-meta.yaml` now distinguishes planned
   ownership from actual service. `scenarioOwners` plus `operationOwners`
   resolve `servingArtifact`, the immutable unit responsible for closing the
   route; `servedOperations` resolves the optional `servedArtifact`, which is
   emitted only when a runnable production composition really mounts the
   operation. The first value drives ownership and selection and is not mount
   proof. `graph verify` reports `aex-route-unserved` for every route with no
   actual mount, rejects malformed and cross-plane owners, and checks service
   mount sets against the generated actual projection. The account read remains
   planned for `central-identity-api` but unserved; the identity process mounts
   only Auth. Session telemetry export admission is likewise honestly absent,
   and `regional-session-api` currently serves exactly 15 operations.

   Scenario selection is also executable now: an `observes` edge is necessary
   but insufficient. Every scenario must name a namespaced Cargo/npm `package`
   and exact `target`, and the package must claim both in its AEX metadata. The
   route workflow emits and downstream lanes consume a scenario matrix carrying
   those exact claims. Existing scenario rows intentionally have no invented
   runnable claims, so the graph remains red until real targets land.

   Freshness starts from authored OpenAPI/route metadata rather than optional
   generated sentinels. Deleting both `bundle.json` and `routes.json`, or
   deleting `routes-meta.yaml` while outputs remain, is therefore a failure.
   Both planned and actual delivery metadata remain outside `bundle.json`, so
   neither can mint a new public wire identity.
4. **`migrations/central/*.sql`** with an `-- aex-migration: tx= destructive= phase=`
   header on every file, `grants.toml` for privileges, and `bundle.lock.json`
   produced by `aex-release-tool migration bundle`. A `GRANT` inside a
   migration body is rejected; a non-transactional migration without a
   `.repair.sql` sibling is rejected.

   **Settled.** The identity and finance streams took the privilege decision
   this gate was waiting on. All 28 `migration-inline-grant` violations are gone:
   `grants.toml` is `schema_version = 2` and now declares the database-wide
   denial, per-role `CONNECT`, schema `USAGE`, table privileges and function
   `EXECUTE`, which `central_schema_admin::grants::GrantSet::render` turns into
   ordered SQL. `migrations/central/bundle.lock.json` exists, so
   `migration-bundle-missing` is cleared and the central bundle digest is real.
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

## 3.6 The local artifact run

The pipeline was driven end to end on one machine, publishing nothing. What it
produced, and what it could not, is the most useful thing this section says.

**Built and described.** The two `TypeScript` edges. `bun build` produced the
bundles, `artifact package` produced byte-stable ZIPs whose single member is the
`handler.js` each unit declares, and `artifact describe` produced envelopes over
real digests, real sizes, a real toolchain and a real input closure.

| unit | artifact digest | bytes | closure |
| --- | --- | --- | --- |
| `stripe-command-edge` | `sha256:d3756c44bcf25e6c400498e944894b1005b486646affab525e152d1ea4601145` | 218697 | 91 files |
| `stripe-webhook-edge` | `sha256:5726dd3e3a53a469c5ed870909c1280bc6f9b2697e642a233ce5124b5862c46b` | 241639 | 90 files |

**One earned receipt.** `evidence new` over this crate's own `nextest` `JUnit`
report: 317 declared, 317 collected, zero skipped, zero flaky, verdict derived
rather than declared. `evidence attach` hashed the report onto it.

**The composition.** `manifest new` refuses any deployable that is neither
described nor recorded, so the manifest that exists names 2 units and carries 29
recorded absences. It validates, it passes the strict environment scan, and
`manifest diff` against a one-unit predecessor reports the added unit and
nothing else.

**The gate nobody can earn here.** No Rust Lambda `bootstrap` was produced. The
aarch64 cross-build does not complete on Windows: zig 0.16.0's `cc` wedges while
building `aws-lc-sys 0.43.0`, once on
`third_party/s2n-bignum/.../arm/aes/aes-xts-enc.S` and once on a `-E`
preprocessor probe, under `cargo lambda build`, under
`cargo lambda build --compiler cargo-zigbuild`, under `cargo zigbuild`, and
under plain `cargo build` with `CC_*`, `AR_*` and the linker pointed at zig by
hand. Two host faults were found and fixed on the way — `cargo lambda`'s own
`zig cc` wrapper resolves zig by running `python3 -m ziglang version`, and this
host's `pyenv-win\shims\python3.bat` never returns — and the wedge outlived
both. `AWS_LC_SYS_NO_ASM=1` is refused for a release profile, and the CMake
builder needs a generator this host does not have (no `ninja`, no `make`, and
Visual Studio cannot target `aarch64-linux`). A Linux or macOS runner, or a
container, is the fix; there is no way to produce those bytes here and nothing
to fabricate them from.

## 4. What was deliberately left undone

1. **OCI and rootfs packaging.** `artifact package --form oci|rootfs` returns a
   typed refusal. An OCI image is assembled over a digest-pinned base whose
   blobs come from a registry, and a rootfs comes from the Hands image build;
   neither is producible offline. `artifact plan` still prints the exact build
   invocation for both, and the Dockerfile scanner enforces COPY-only.
2. ~~**`artifact describe`.**~~ Wired. It fills every field a local build
   establishes and marks the rest `unearned` rather than inventing a run id
   nobody issued; see §1. What is still undone is the CI half: an envelope
   produced here can never verify, and that is the intended state.
3. ~~**`evidence new`/`attach` and `verification new` as subcommands.**~~
   Wired. `evidence new` takes a run context plus a `JUnit` report and derives
   the inventory from element occurrence and the conclusion from the inventory,
   so a lane cannot write its own verdict. `declared` is supplied by the caller
   from the runner's listing rather than read back out of the report, because
   deriving it from the report would make `collected == declared` true by
   construction. `evidence attach` hashes the file itself. `verification new`
   wraps `new_statement`.
4. **The DynamoDB ledger store.** `LedgerStore` is a trait; the JSONL mirror is
   the only implementation. Fence and transition semantics are identical, so
   the DynamoDB adapter is a store, not a rewrite.
5. ~~**`manifest new`/`diff`**~~ and **`plan bind`**. `manifest new` assembles a
   composition from a set of envelopes and refuses any deployable that is
   neither described nor recorded in an unearned ledger, so an incomplete
   release cannot be mistaken for a complete one. `manifest diff` takes two
   manifests as files rather than reading the previous one from an object store
   that does not exist. `plan bind` still needs that store.
6. **`test-registry`, `flake scan` and `workload verify`.** By orchestrator
   ruling these live in `aex-workspace-check`, not here. `aex-release-tool`
   depends on that crate and consumes `release/test-registry.json` and
   `release/unearned-evidence.json` through its own types
   (`src/test_registry.rs`). Nothing is re-implemented; the rule ids and
   messages in `aex-workspace-check` are the authority.

   **`janitor sweep` is implemented, here rather than in
   `aex-workspace-check`.** OD-36 makes reclamation a release gate, and the
   gate is the evidence receipt's `residue` field, whose type lives in this
   crate; putting the sweep beside the receipt it feeds is what let
   `DataBlock::from_sweep` become the field's first production constructor.
   The tag vocabulary, the reclamation order and the reclaimable resource
   table are data in `release/policy/test-profiles.toml`, read through
   `aex_workspace_check::policy::Policy::embedded()`, so the crate that stamps
   the tags (`aex-test-harness`) and the tool that sweeps by them cannot
   drift. What is **not** implemented is the credentialed reclamation adapter:
   discovery against a live plane needs a deployed plane and a dedicated
   identity, which OD-07 keeps out of scope. `janitor sweep --mode reclaim`
   therefore refuses per resource with the required steps named and exits `41`,
   rather than reporting a green sweep that removed nothing. The engine, the
   guard, the order and the report are complete; the adapter is one
   `Reclaimer` implementation away.
7. **Live publication.** Every publish step in `main.yml` and every apply step
   in `_release-engine.yml` refuses with a stated reason and a classified exit
   code rather than pretending. The lanes are wired end to end; the buckets,
   repositories, roles and GitHub Environments are not.

The reusable Rust lane also owns one feature-specific target that an ordinary
package build cannot discover: whenever `aex-session-dynamodb` is selected it
checks, clippies and tests the crate with only
`capacity-limit-projection-write`. The same job compiles central control under
its declared dependency and runs the `authz-projection-write` compile-fail
boundary proof, so feature unification cannot silently hand central control the
regional capacity producer.

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
