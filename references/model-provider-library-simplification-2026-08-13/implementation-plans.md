---
title: Model-provider library simplification — implementation plans
description: Six-phase implementation plans compiled from the 2026-08-13 spikes: ordering, per-phase touchpoints, verification gates, parallel lanes, and cut points.
keywords:
  - implementation
  - plan
  - rig
  - models.dev
  - simplification
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-13
related:
  - references/model-provider-library-simplification-2026-08-13/design-2026-08-13.md
  - references/model-provider-library-simplification-2026-08-13/spikes/deletion-touchpoints.md
---

# Implementation plans

Six phases, ordered by the spike-derived constraint: **move ports → land the
rig adapter + generated catalog + rewiring → delete the gateway and catalog
authority → CI/release edits → live harness + tokenizers → infra teardown +
docs.** Reversing any of the first three breaks custody or brain-mux
mid-sequence. Phases 1a, 1b, and 3 partially parallelize; Phases 2 and 4 are
serial after their inputs. Every phase ends with the workspace gates:
`cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo nextest run` for touched crates, `cargo check --workspace
--all-targets`, and `cargo run -p aex-workspace-check`.

## Phase 1a — Relocate the credential ports (unblocks gateway deletion)

Move into `crates/aex-brain-provider-custody/src/credential.rs` from the
gateway: `ProviderCredentialDirectory`, `ProviderCredentialDecryptor`,
`ProviderCredentialBinding`, `ProviderApiKey`, `CredentialResolveError`,
`CredentialRevision`, `BindingState`, `DenyAll*`. The decrypted-key
`CredentialCache` and the single-flight registry do **not** move here — they
are consumer-side optimizations and land in `aex-brain-provider` (Phase 2),
keeping custody a stateless adapter. Custody's `authority.rs` imports repoint
to the local module; drop the gateway dependency from its Cargo.toml; keep
`live_suite = "aex-live-brain-mux"`.

Verify: custody unit/security tests green with zero gateway references
(`cargo nextest run -p aex-brain-provider-custody`); grep confirms no
`aex_brain_provider_gateway` import anywhere in custody.

## Phase 1b — Vocabulary split + slim catalog + vendored codegen (parallel with 1a)

1. **Split the vocabulary crate.** Create `crates/aex-model-vocabulary`
   (pure): move `canonical.rs`, `failure.rs`, `primitives.rs` from
   `aex-model-catalog`. Mechanical import renames across the ~9 consumer
   crates (`aex_model_catalog::canonical` → `aex_model_vocabulary::canonical`
   etc.).
2. **Slim `aex-model-catalog`.** Delete `signature.rs`, `collection.rs`,
   `receipt.rs`, `catalog.rs`, `build_binding.rs`, `tests/properties.rs`;
   slim `document.rs` (minimal `ModelLimits`, 2-bit `Capability`/
   `CapabilitySet`, `DurableOperationSupport` → moves to aex-brain-domain),
   `qualified.rs` (thin wrapper + `admit`), `wire_pending.rs`
   (`CatalogRevision` = blake3 of the vendored snapshot), `fixture.rs` (keep
   `qualified`/`entry` signatures). Drop `aws-lc-rs`, `sha2`, `bytes`;
   `serde_json` → dev.
3. **Delete the unqualified arm.** `CatalogError` becomes two-arm
   (`UnknownProvider`, `UnknownModel`); delete
   `unqualified_provider_model` from `api/schemas/registries/errors.yaml`
   and regenerate contracts (`aex-contract-gen`); drop session-app's third
   `QualificationRefusal` arm and the scripted-test expectations.
4. **Vendor models.dev + codegen, manual refresh.** Vendor
   `models.dev/api.json` under `release/models-dev/` + committed
   `scripts/models.digest`; write `scripts/gen-models.ts` (bun, ~150 LOC)
   emitting the checked-in module `crates/aex-model-catalog/src/generated/
   admit.rs` (801 rows, `GENERATED_FROM_SHA256`) plus `scripts/
   fetch-models-dev` (manual fetch + regenerate helper, ~35 LOC). CI
   verifies: `sha256sum -c` + regenerate + `git diff --exit-code`. **No
   cron, no PR bot** — a maintainer bumps the snapshot when needed; the
   change rides the normal release train.
5. **Repoint remaining consumers** per the minimal-surface spike
   (`aex-brain-app` −2 trait methods + `support()` → `None`; session-app
   ports.rs copy deleted, direct dep; brain-domain gains
   `DurableOperationSupport`; test-support 0 edits). `mc1_` rendering
   unchanged.

Verify: `cargo check --workspace --all-targets`; touched-crate test suites
green; `aex-workspace-check` passes (member count change, registry regen);
`aex-contract-gen check` clean.

## Phase 2 — `aex-brain-provider` + composition rewiring (serial after 1)

1. New crate `crates/aex-brain-provider`: depends on rig-core (pinned
   0.41.0, default minus `agent`), custody, brain-app, brain-domain, the
   slimmed catalog, the vocabulary crate. Implements `ProviderPort` **whose
   `budget` parameter is removed** — the trait signature change ripples to
   every impl (brain-mux `AbsentProvider` + wake_tests test providers,
   activation/memory.rs fixtures, tests/ports.rs, activation/tests.rs
   providers) and the mux's per-activation 1 MiB stream-buffer permit
   (BR-59) is deleted. The adapter enforces no bounds.
   - Owns the decrypted-key `CredentialCache` + single-flight (moved from
     the gateway, consumer side).
   - Dialect routing: generated `DialectClass` → rig native client or
     OpenAI-compatible builder (`base_url` from generated `ProviderMeta`,
     vercel constant).
   - Per-dispatch client: shared `reqwest::Client` injected via
     `http_client()`, key by borrow (`key.as_str()`).
   - Pre-send: `admit()` validation, revocation fence (`revalidate` +
     epoch check) with `invalidate_binding` on mismatch — `NotSent` proof.
   - Dispatch + bounded retry: ≤3 attempts, equal-jitter, cancellable, on
     429/503 definitive refusals and transport drops; **exhaustion returns
     `DispatchStage::Terminal`** so the brain settles `KnownFailure` and the
     run stops (`PossiblySent` after a send; proofs per the second-round
     disposition 2).
   - Streaming: first `Ok(item)` → `mark_response_started`
     (`ResponseStarted`); map `StreamedAssistantContent` →
     `canonical::` blocks; `Final` usage → `NormalizedUsage`;
     `seal()` → `CompleteAssistantMessage`; assemble `ProviderReceipt`
     (request ids from successful responses only; `retry_after: None`).
     **No interim-usage preview frames.**
   - `resolve_unknown` → `NoDurableOperation`.
   - Gateway admission is broad: the generated gateway table is the
     allowlist; `ModelId` validates against it.
   - Tests: mock-server golden per dialect (frame → canonical mapping),
     proof-value tests, 429/503 + drop retry counters, leak tests rewritten
     for the new boundary.
2. brain-mux: `release_catalog.rs` → allowlist loader (const non-empty
   assert replaces `is_service_capable`); `build.rs` keeps task-shape only;
   `wake.rs` wires the new router constructor; delete the `catalog_verified`
   readiness flag chain (control.rs/health.rs/compose.rs/dependencies.rs/
   pressure.rs — exact lines in the deletion spike); `retained_pins()` →
   single compiled pin.
3. session-stream-api: delete `release_catalog.rs`'s signed-collection
   half; `ModelQualifier` impl over the slimmed catalog `admit()`.

Verify: `cargo nextest run -p aex-brain-provider -p brain-mux -p
aex-session-app -p session-stream-api`; workflow-validator tests updated;
a dry-run `aex-release-tool artifact plan` for brain-mux/session-stream-api
without any `AEX_MODEL_CATALOG_*` env succeeds.

## Phase 3 — Delete the gateway + catalog-authority plumbing (serial after 2)

- Delete `crates/aex-brain-provider-gateway`; confirm custody, brain-mux,
  and the new adapter no longer reference it.
- Delete `infra/modules/model-catalog-authority/` (public) and the release
  lane: `model-catalog-publish.yml`, `model-catalog-qualify.yml`,
  `scripts/cicd/model-catalog-qualification-binding.mjs`, the two
  `scripts/validate/model-catalog-*.test.ts`; edit
  `_compile-artifacts.yml` (drop binding steps; keep tool-catalog step),
  release-tool (`artifact.rs` model-catalog removal **and tool-catalog env
  decoupling**, `main.rs`, `oci.rs`, `describe.rs`,
  `composition_inputs.rs` + tests), the two validator test files,
  `release/README.md`, `release/model-catalog/` source directory,
  `seams.toml` reword. Regenerate `test-registry.json` +
  `unearned-evidence.json`.

Verify: CI-lane validator suite green (`bun test` in `scripts/validate`);
`_compile-artifacts.yml` path re-validated via a mock release-tool plan;
workspace-check member/registry rules green.

## Phase 4 — Live harness + dependency prune (serial after 3)

Delete `tests/live/aex-live-model-catalog/`; remove the `tokenizers`
workspace dependency (root Cargo.toml:256) and regenerate Cargo.lock; keep
`tests/live/aex-live-brain-mux/` untouched (seam claims remain valid through
the new adapter; verified it consumes no provider keys, so all
`AEX_LIVE_PROVIDER_KEY_*` envs and the qualifier's budget envs die with the
harness).

Verify: `cargo check --workspace --all-targets` + lockfile diff shows the
tokenizers tree gone; live-brain-mux manifest/registry rows still resolve.

## Phase 5 — Platform repo + operations (serial after 3)

Cross-repo, applied inside the platform repository per
`workspace-structure.md`: delete the composition root
`platform/infra/terraform/composition/roots/model-catalog-authority/` and
`platform/scripts/validate/model-catalog-authority.test.ts`; edit
`platform/.github/workflows/ci.yml`, `platform/infra/README.md`,
`platform/infra/terraform/composition/README.md`,
`platform/references/repo.md`, `platform/references/README.md`,
`platform/release/READINESS.md`. Operations: delete the two GitHub
Environments and their variables/secrets (including
`AEX_MODEL_CATALOG_BINDING_JSON`), `terraform destroy` the composition root,
schedule the KMS key deletion window.

Verify: platform CI (`bun test`, `terraform fmt -check` matrix) green with
the rows removed; AWS inventory shows no model-catalog-authority resources.

## Phase 6 — Docs sweep (parallelizable with 5)

Per the deletion spike's Phase 6 list: delete
`references/model-catalog-authority.md`; edit `develop.md`,
`rewrite/providers.md` (rewrite or superseded banner), `rewrite/brain.md`,
`rewrite/delivery.md`, `rewrite/test-architecture.md`,
`rewrite/architecture-performance-v1.md`, `rewrite/implementation/
brain-activation.md`, `rewrite/contracts.md`, `architecture.md`,
`backlog.md`, `glossary.md`, `references/README.md`, `repo.md`; update the
08-12 proposal's status; leave dated point-in-time records untouched.
Add `rig`/`rig-core`/`rig-agent` to `aex-workspace-check`'s vendor list.

Verify: docs gate green (`bun test` in `scripts/docs`), workspace-check
green, no dangling `related:` targets.

## Lanes and cut points

| Lane | Phases | Cut point (each independently revertible) |
| --- | --- | --- |
| A: custody ports | 1a | custody tests green before anything else moves |
| B: vocabulary split + catalog slim + codegen | 1b, 4 (part) | workspace builds with the slimmed catalog and old gateway still wired |
| C: aex-brain-provider + mux rewiring | 2 | `ProviderPort` swapped behind a feature/env switch before gateway deletion (optional safety) |
| D: deletion + release lane | 3 | CI validator suite green |
| E: infra + ops + docs | 5, 6 | platform repo merge independent of aex merge |

Total estimated surface: ~2,800 new lines (aex-brain-provider ~1,200 incl.
tests, generator ~150, fetch script ~35, allowlist module ~850 generated,
vocabulary crate ~100 scaffold), ~26k deleted lines (gateway 25.6k + catalog
authority modules), and ~40–60 edited lines in the brain seam plus mechanical
vocabulary import renames. The generated file is checked in and never edited
by hand.

## Deferred (recorded, not in any phase)

- Capability matrix product surface — only if a product requirement forces
  it.
- Runtime emergency-disable denylist — decision 8 stands (rebuild-and-
  redeploy); the model-withdrawal procedure is written into
  `failure-response.md` during Phase 6.
