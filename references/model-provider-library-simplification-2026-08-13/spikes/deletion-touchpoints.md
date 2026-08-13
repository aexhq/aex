---
title: Spike — deletion and rewiring touchpoint inventory
description: The complete phase-grouped inventory for decision A: code, CI/release, infra (public + platform), live harness, docs, and structural rules.
keywords:
  - deletion
  - touchpoints
  - release
  - spike
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-13
related:
  - references/model-provider-library-simplification-2026-08-13/implementation-plans.md
  - references/model-provider-library-simplification-2026-08-13/design-2026-08-13.md
---

# Spike: deletion and rewiring touchpoint inventory

Full file-level inventory for decision A, condensed here by phase. Paths are
relative to `aex/` unless prefixed `platform/` or `../`.

## Phase 1 — Catalog authority code (crates/aex-model-catalog)

- **DELETE**: `src/signature.rs`, `src/collection.rs`, `src/receipt.rs`,
  `src/catalog.rs`, `build_binding.rs`, `tests/properties.rs` (suite moves
  with `canonical.rs` where relevant).
- **REWRITE**: `src/qualified.rs` (thin wrapper + `admit`), `src/document.rs`
  (minimal limits + 2-bit capabilities + `DurableOperationSupport`),
  `src/wire_pending.rs` (`CatalogRevision` from snapshot digest),
  `src/fixture.rs` (keep `qualified`/`entry` signatures; delete
  `promote`/`document`/`canonical_bytes`/map builders), `Cargo.toml`
  (drop aws-lc-rs, sha2, bytes; serde_json → dev).
- **KEEP**: `canonical.rs`, `failure.rs`, `primitives.rs` — consumed by
  brain-app, brain-domain, brain-tool-catalog, brain-hands, brain-managed-web,
  session-app, test-support.
- Consumers to repoint: `aex-brain-app` (ports/catalog.rs, ports/provider.rs,
  activation/memory.rs, tests), `aex-brain-domain` (ids.rs, wire_pending.rs,
  journal.rs, context.rs, tests), `aex-brain-tool-catalog`, `aex-brain-hands`,
  `aex-brain-managed-web` (drop optional dep), `aex-brain-test-support`,
  `aex-session-app` (delete ports.rs copy; direct dep), `services/
  session-stream-api`, `runtimes/brain-mux`.

## Phase 2 — Gateway deletion and rewiring

- **DELETE** `crates/aex-brain-provider-gateway` (whole crate: 24 modules,
  build.rs, both test files). Consumers: brain-mux, custody, live harness
  (dies).
- **Credential ports move to `aex-brain-provider-custody`** first (ordering!):
  `ProviderCredentialDirectory/Decryptor`, `ProviderCredentialBinding`,
  `ProviderApiKey`, `CredentialResolveError`, `CredentialRevision`,
  `BindingState`, `CredentialCache` (+ flight registry if the rig router
  wants it). Custody gains no new deps; no cycle.
- **brain-mux**: `src/release_catalog.rs` rewritten to the allowlist loader;
  `build.rs` keeps task-shape generation, drops the `build_binding`
  `#[path]` include and the `generate("brain-mux")` call + p256/sha2
  build-deps; `Cargo.toml` swaps gateway+model-catalog deps for the rig
  adapter; `src/main.rs` (bind_release_catalog, retained_pins→single pin,
  catalog_verified call), `src/wake.rs` (router construction at 1028,
  readiness strings PROVIDER_GATEWAY_ABSENT/CATALOG_ABSENT/CATALOG_NO_ACTIVE_
  MODELS, AbsentCatalog), `src/wake_tests.rs` (fixtures on generated table),
  `src/inline_tools.rs`/`tool_exec.rs` (repoint), `src/control.rs` +
  `health.rs` + `compose.rs`/`dependencies.rs`/`pressure.rs` (delete the
  catalog_verified readiness flag; new semantics: allowlist non-empty
  compile-time assert).
- New crate `aex-brain-provider-rig`: `ProviderPort` impl + production
  `CatalogPort` binding + six `provider.*.stream` seam declarations.

## Phase 3 — CI / release

- **DELETE**: `.github/workflows/model-catalog-publish.yml`,
  `.github/workflows/model-catalog-qualify.yml`,
  `scripts/cicd/model-catalog-qualification-binding.mjs`,
  `scripts/validate/model-catalog-authority.test.ts`,
  `scripts/validate/model-catalog-qualification-binding.test.ts`,
  `release/model-catalog/` (source + README).
- **EDIT**: `_compile-artifacts.yml` (drop the binding steps 127-198 and
  `--require-model-catalog` 205-208; keep the tool-catalog step 119-125);
  `tools/aex-release-tool/src/artifact.rs` (remove all
  `ModelCatalog*`/`requires_model_catalog`/`plan_with_model_catalog`;
  **decouple `AEX_TOOL_CATALOG_SHA256` from the all-or-none 5-variable block**
  — lines 413-441 — or the tool binding breaks), `main.rs` (flag removal),
  `oci.rs` (env closure 258-283), `describe.rs` (identities.catalogs.model
  → snapshot digest), `composition_inputs.rs` (catalog authority → snapshot
  digest identity + tests), `scripts/validate/hosted-release-ownership.test.ts`
  (23-24), `scripts/validate/public-release-publication.test.ts` (399-462),
  `release/README.md` (58), `release/policy/seams.toml` (rewrite local proof
  text for the six provider rows).
- **REGENERATE**: `release/test-registry.json`, `release/unearned-evidence.json`
  (via `aex-workspace-check registry build` / `bun run generate`).

## Phase 4 — Live harness

- **DELETE**: `tests/live/aex-live-model-catalog/` (whole crate), the
  `tokenizers` workspace dependency (root Cargo.toml:256) and its lockfile
  tree (sole consumer was the qualifier's tokenizer oracle).
- **KEEP** `tests/live/aex-live-brain-mux/` — it is the brain-mux live
  companion (smoke + brain-core load workloads), not a gateway harness;
  8 packages declare it as `live_suite`. Its `provider.*.stream` seam claims
  stay valid through the rig adapter.

## Phase 5 — Infra (public + platform) and operations

- **Public**: DELETE `infra/modules/model-catalog-authority/` (main.tf,
  variables.tf, outputs.tf, versions.tf, aex.toml, README, tftest, lockfile).
- **Platform** (cross-repo, easy to miss):
  DELETE `platform/infra/terraform/composition/roots/model-catalog-authority/`
  (6 files) and `platform/scripts/validate/model-catalog-authority.test.ts`;
  EDIT `platform/.github/workflows/ci.yml` (bun test list, two terraform
  fmt-check targets, matrix row), `platform/infra/README.md`,
  `platform/infra/terraform/composition/README.md`,
  `platform/references/repo.md`, `platform/references/README.md`,
  `platform/release/READINESS.md`.
- **Operations**: delete GitHub Environments `aex-model-catalog-publisher` +
  `aex-model-catalog-qualifier`, their variables/secrets (incl.
  `AEX_MODEL_CATALOG_BINDING_JSON`); `terraform destroy` the composition
  root (KMS key + OIDC role) and remove the tfstate object; schedule the KMS
  deletion window.

## Phase 6 — Docs and structural

- **DELETE**: `references/model-catalog-authority.md`.
- **EDIT**: `develop.md`, `rewrite/providers.md` (large rewrite or superseded
  banner), `rewrite/brain.md` (peer-asks + production peer set wording),
  `rewrite/delivery.md`, `rewrite/test-architecture.md`,
  `rewrite/architecture-performance-v1.md`, `rewrite/implementation/
  brain-activation.md`, `rewrite/contracts.md`, `architecture.md`,
  `backlog.md` (drop deferred live-receipts work), `glossary.md`,
  `references/README.md` index, `repo.md` (infra wording).
- **NO CHANGE**: `rules.md`, `contributing.md`, `repository-hygiene.md`,
  workspace-root `references/` and `AGENTS.md` files, `release/path-map.toml`,
  `release/policy/test-profiles.toml`, `workload-registry.toml`,
  `test-images.toml`, `units.toml`.
- **Structural**: root `Cargo.toml` members −3 (+1 rig crate); `tools/
  aex-workspace-check/src/inventory.rs` (CRATES + LIVE_TARGETS rows),
  `errorname.rs` (3 path annotations), `rules.rs`
  `is_vendor_dependency` **add `rig`, `rig-core`, `rig-agent`**. Naming: avoid
  reusing the `-gateway` suffix for the new crate. Cargo.lock: +30–60 entries
  (rig graph), −15–25 (tokenizers tree), net ~100–150 lines.

## Ordering constraint

Move credential ports → land rig adapter + generated catalog + rewiring →
delete gateway + catalog-authority code → CI/release edits + registry regen →
live harness + tokenizers → platform/infra teardown + docs. Reversing any of
the first three breaks custody or brain-mux mid-sequence.
