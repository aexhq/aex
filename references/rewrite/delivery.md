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

### 2026-08-03 clean-cut ownership ledger

D19/D20 ownership is now mechanically split. Public `aex` owns immutable,
plane-neutral build/publication inputs and the release contract vocabulary;
private `platform` owns every binding value and is the only manual hosted
release entry point. The public `release.yml` and `_release-engine.yml` were
removed, and `assurance.yml` no longer accepts or resolves a plane. A structural
test refuses their return, any public `binding_ref`, private-repository checkout,
hosted environment, or AWS credential step.

The public contract set now also includes `aex.resolved-placement.v1` and
`aex.saved-plan-envelope.v1`. Typed validation rejects self-digest tampering,
plaintext credential shapes, mutable secret versions, unsafe endpoints,
unsorted regions, stale or over-60-minute plans, mutable workflow refs and
opaque-plan byte mismatches. The saved-plan envelope binds the private source
and public module bundle, release tool and Terraform binary bytes, provider lock
and packages, fixed runner paths, encrypted plan object and KMS identity, and an
explicit present/absent state snapshot with backend configuration identity.
No artifact was published and no hosted system was touched by this slice.

### Non-circular artifact evidence identity

`envelopeDigest` remains the self-digest of the complete artifact envelope,
including its receipt references. Receipts do not bind to that digest: doing so
would require a receipt to predict the digest of an envelope that does not yet
contain the receipt. Artifact-scoped freshness now binds
`subject.artifactSubjectDigest` instead.

`artifactSubjectDigest` is RFC 8785 canonical JSON over an explicit
`aex.artifact-subject.v1` projection: unit, media, exact clean source repository,
commit and ref, the complete build-input block, and the complete output byte
identity. The output projection includes size, target, symbols and every OCI
manifest/config/layer digest. It excludes the workflow execution, publication
location, scanner/provenance verdicts and receipt references. A local draft can
therefore mint the subject immediately after packaging. `evidence bind-artifact`
verifies an already-earned receipt and the recomputed draft subject, requires
its repository, commit and unit scope to agree, refuses rebinding, sets only
`artifactSubjectDigest`, and reseals it to a new auditable receipt digest.
Certification applies the exact workflow and immutable location, recomputes the
subject, refuses any drift, verifies every artifact-bound receipt names it
exactly, inserts the receipt refs, then seals the complete envelope. It never
silently binds or rewrites evidence.

The exclusions are deliberate. Workflow/run identity remains an independent
exact receipt and envelope binding, and the final envelope still binds the
content-addressed publication location. Neither changes the bytes or input
closure a test or scanner observed. Supply-chain verdicts are evidence about
the subject, never inputs to its identity. Computing one small canonical
projection in addition to the full envelope digest is bounded by envelope size
and does not hash artifact bytes again.

This identity change earns no evidence by itself. CI must still run each real
per-unit producer and pass its sealed receipt to `artifact certify`; missing,
misbound, stale or failing receipts remain refusals.

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
| `schemas` | The seven release JSON Schemas, embedded. |
| `release_contract` | Typed environment-binding, resolved-placement, and saved-plan-envelope validation for the private hosted-release consumer. |
| `selftest` | Determinism and monotonicity probed against the real graph. |
| `test_registry` | Reads `release/test-registry.json` and `release/unearned-evidence.json` through `aex-workspace-check`'s own types. Derives nothing; asserts the delivery graph and the registry agree on the live set. |

Subcommands landed: `graph build|verify|select|explain|diff|matrix`,
`artifact recipes|plan|package|describe|verify|publish-plan`,
`manifest new|diff|digest|validate|order|rollback-candidates`,
`evidence new|attach|verify|require|aggregate`,
`verification new|verify`, `admit`,
`plan summarize|policy`, `ledger append|list|verify`, `private-path check`,
`migration bundle|verify`, `policy terraform|workflows|dockerfile`, `schema`,
`contract validate`,
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

Seven schemas, authored here per D14-05 so the contract generator needs no
special case: `artifact-envelope.json`, `composition-manifest.json`,
`verification-statement.json`, `environment-binding.json`,
`evidence-receipt.json`, `resolved-placement.json`, and
`saved-plan-envelope.json`. The contracts stream had not created that directory,
so there is no conflict to record.

### `release/`

`units.toml` (29 deployables), `scenario-ownership.toml` (20 scenarios),
`path-map.toml` (47 rules, zero orphans against the current tree),
`release/policy/artifact-policy.toml`, `release/policy/freshness.toml`,
`release/policy/terraform-policy.toml`, `release/policy/rollout-policy.toml`,
`release/policy/private-path-policy.json`.

### `.github/workflows/`

The old `ci.yml` and `live-user-tests.yml` are deleted, not aliased. Ten
files: three lane classes (`pr`, `main`, `assurance`) and seven
reusable workflows (`_route`, `_rust-lane`, `_node-lane`, `_terraform-lane`,
`_scenario-lane`, `_build-artifacts`, `_receipts`). Hosted release orchestration
lives only in the private repository.
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

The 2026-08-02 count of 148 graph violations and the later raw-gap count are
superseded by the explicit-deferral correction below. The current authorities
carry 49 exact unmounted-route deferrals and 20 exact non-runnable-scenario
deferrals. `graph verify` is green because every incomplete state is explicit,
non-empty, mutually exclusive with a served/runnable claim, and included in its
machine summary. This is structural accounting, not a release receipt:
deferred scenarios are absent from execution matrices, and artifact
certification plus environment admission still require real evidence.

## 3. What every stream owes me

`graph verify` is the structural gate. It rejects every implicit or
contradictory gap and reports explicit prelaunch debt in its summary. Artifact
certification and environment admission remain the evidence gates.

### 3.1 `[package.metadata.aex]` — landed

All manifests carry ownership metadata. `aex-live-model-catalog` names
`brain-mux` as its deployable: the catalogue is loaded and enforced inside that
runtime rather than shipped as a standalone service. The former reference to a
nonexistent artifact-metadata manifest is removed, so the live companion
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
   proof. An unmounted route must instead have an exact non-empty
   `deferredOperations` reason; absence of both is `aex-route-unserved`, and a
   served+deferred conflict also fails. The verifier rejects malformed and
   cross-plane owners and checks service mount sets against the generated actual
   projection. The account read remains planned for `central-identity-api` but
   explicitly deferred. Session telemetry export admission is likewise honestly
   deferred, and `regional-session-api` currently serves exactly 17 operations.

   Scenario selection is also executable now: an `observes` edge is necessary
   but insufficient. Every scenario must name a namespaced Cargo/npm `package`
   and exact `target`, and the package must claim both in its AEX metadata. The
   route workflow emits and downstream lanes consume a scenario matrix carrying
   those exact claims. Existing scenario rows have no invented runnable claims;
   they carry non-empty `deferred` reasons, remain visible in the graph summary,
   and are excluded from the scenario matrix until real targets land. Missing,
   partial, or runnable+deferred claims still fail verification.

   Freshness starts from authored OpenAPI/route metadata rather than optional
   generated sentinels. Deleting both `bundle.json` and `routes.json`, or
   deleting `routes-meta.yaml` while outputs remain, is therefore a failure.
   Both planned and actual **placement** metadata remain outside `bundle.json`,
   so neither can mint a new public wire identity. *Whether* a route is served
   does enter the bundle, as a boolean `deferred` flag: moving an operation
   between deployables changes nothing a caller can observe, while an operation
   that answers `501` instead of a resource is a materially different published
   surface, and a digest identical across the two would identify a subset of the
   contract rather than the contract. The ledger's prose reason stays out.
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

7. **Complete composition handoff.** `manifest inputs` now derives the public
   composition identities from the exact protected run, acquired release
   assets and checked-out source. It byte-compares generated module/regional
   bundles, rebuilds the central migration lock, derives the provider closure,
   binds the source tool-catalogue snapshot to the certified Brain envelope,
   and refuses a registry package without a real publication identity. The
   regional generator now authors generation `1` in its canonical bundle.
   The release remains a draft until `main.yml` verifies all certified
   envelopes, emits and attests the manifest/store, uploads them without
   replacement, reads back identical bytes and exposes the prerelease. Unit
   builders still emit only `draft-envelope.json`, so missing real per-unit
   certification remains a visible exit-`40` blocker. Hosted planning and
   apply remain private responsibilities.

### 4.1 Exact remaining composition-handoff blocker

`CompositionInputs` is not a convenient summary that may be reconstructed from
plausible values. Each member must come from the producer that earned it. This
is the authority map for the first public Rust-native candidate:

| Input                 | Existing authority                                                                                                         | Candidate status                                                                                                                                                         |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `contractDigest`      | `api/generated/bundle.lock.json`, checked by `aex-contract-gen`                                                            | Available.                                                                                                                                                               |
| `source`              | `GITHUB_REPOSITORY`, `GITHUB_SHA`, `GITHUB_RUN_ID` and `GITHUB_RUN_ATTEMPT`, cross-checked against the unique release tag  | Available only inside the publishing workflow.                                                                                                                           |
| `releaseTool`         | Rebuilt deterministic executable plus digest, size, version, HTTPS release URI and verified GitHub attestation             | Available.                                                                                                                                                               |
| `packages`            | Registry version, integrity and provenance from the package publisher                                                      | The hosted registry currently has no `npm-package` unit, so the producer emits an explicitly verified empty map; adding one makes composition fail until its publisher supplies identities. |
| `migrations.central`  | Canonical `migrations/central/bundle.lock.json` and the certified `central-schema-admin` envelope                          | The producer rebuilds and byte-checks the lock, and refuses unless the certified admin envelope carries the same bundle digest. Embedding those bytes in the worker image remains required. |
| `migrations.regional` | Published `regional-tables.json` transport identity plus an authored monotone table generation                             | Produced from the acquired/tracked byte-identical bundle, including authored generation `1`.                                                                             |
| `infra`               | Published module-bundle identity plus one exact Terraform/provider lock closure                                            | Produced from a deterministic module rebuild, the exact version in `_terraform-lane.yml`, and every module provider lock; version drift is refused.                     |
| `catalogs`            | Signed catalogue publication identities carried by certified runtime envelopes                                             | Brain build plans now bind model and tool digests; composition requires both from a certified Brain envelope and checks the tool digest against source.                  |
| `policy`              | Canonical digest producers for the artifact and freshness policies, the pinned Rust channel, and the source-policy version | Produced directly from the policy files and pinned toolchain.                                                                                                             |
| unit envelopes        | `artifact certify` over immutable readback, GitHub provenance and artifact-bound passing build/test receipts               | Implemented for all 36 currently published units. Browser MicroVM variants are deferred until a pinned ARM64 browser layer exists. Startup-mode envelopes explicitly defer supply-chain scanners while retaining exact build, catalogue, provenance and publication identity. |
| manifest publication  | Exact manifest bytes, HTTPS release asset identity and GitHub attestation under the workflow the private verifier trusts   | `main.yml` attests and uploads manifest/store before undrafting, then reads both back. The private verifier must pin `main.yml` for these two subjects.                  |

The smallest honest completion sequence is:

1. Make the validation lanes earn every receipt class in `release/units.toml`.
   Download those receipts in the artifact workflow, bind each to the computed
   artifact subject, publish/read back the bytes, and run `artifact certify`.
   Upload exactly one `certified-envelope.json` per registered unit.
2. Expose the manifest URI, blob digest, size, `releaseId` and attestation
   locator as reusable-workflow outputs. Private binding and root-input files
   remain private, separately reviewed inputs; the public workflow must never
   manufacture them.

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
| D-12 | The `assurance` lane exits with a classified code rather than a green no-op where its subject does not exist | A scheduled suite that reports success having checked nothing is worse than one that is red for a stated reason. `admit` then refuses on a missing receipt (exit 40) instead of accepting silence as evidence. Hosted release is private-owned. |
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
9. **Supply-chain assurance is startup-deferred.** Certified unit envelopes
   explicitly record the deferral while retaining exact build, test, catalogue,
   provenance, signature and immutable publication identities. The revisit
   trigger is tracked in `references/backlog.md`; scanner availability does not
   block public composition or dev release.
10. **Private roots still use sibling public-module paths.** A hosted plan may
    satisfy those paths only from the verified module bundle extracted at the
    fixed path recorded in its saved-plan envelope—never by checking out public
    source beside `platform`. The private engine refuses planning until those
    roots are converted.
