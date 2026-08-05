---
title: public repository development
description: Development, testing, environment, documentation, and release routing for the public aex repository.
keywords:
  - development
  - testing
  - dev
  - prd
  - localhost
  - release
audience: implementation agents and maintainers
status: accepted
related:
  - references/rules.md
  - references/repo.md
  - references/contributing.md
  - references/ghcr-visibility-bootstrap.md
  - references/model-catalog-authority.md
---

# Public repository development

Use Bun from the repository root or owning workspace package. `package.json`
scripts are the command source of truth; do not copy a mutable command catalog
into prose.

## Targets

- `dev`: remote non-production hosted plane used for approved live user tests.
- `prd`: remote production hosted plane.
- `localhost`: the developer's machine or local test fixture.

Never call the remote dev plane `local`. Keep real values in gitignored
`.env*` files and load them through approved test/operations tooling.

## Change route

1. Identify the public contract and owning package in [repo.md](repo.md).
2. Write the smallest meaningful blackbox or user test first when practical.
3. Implement in the owning package; update canonical SDK docs for public API
   changes.
4. Run the focused package checks, then the relevant root scripts from
   `package.json`.
5. For docs changes, regenerate and validate the public docs surface.
6. For release changes, verify workflow/candidate integrity tests; do not
   publish or promote manually outside the approved workflows.

The sole manual publication setup is the one-time, fail-closed
[`GHCR public namespace bootstrap`](ghcr-visibility-bootstrap.md). It creates
only digest-addressed package content; normal publication never changes package
visibility.

The signed model-catalog is a separate protected authority. Public main reads
the one canonical `AEX_MODEL_CATALOG_BINDING_JSON` repository variable
documented in [`model-catalog-authority.md`](model-catalog-authority.md); its
absence intentionally leaves the published `brain-mux` gate failing. Do not
populate it with fixtures, an application KMS key, or a moving asset URL.

[`model-catalog-publish.yml`](../.github/workflows/model-catalog-publish.yml)
runs when one reviewed `release/model-catalog/*.source.json` changes on `main`
and also permits exact manual dispatch. It uses the dedicated protected
KMS/OIDC authority and emits a canonical build-binding asset for independent,
atomic installation. It never edits repository variables or secrets, and live
provider monitoring is not a publication input.

## Main-push artifact evidence

Pull requests build and package candidates without publishing. The protected
main workflow additionally preserves exact GitHub provenance, runs the pinned
artifact scanners, publishes content-addressed unit/SBOM/signature assets, and
certifies all 39 deployable rows before composition. `release/units.toml`
declares the required receipt classes and `release/semantic-receipts.json`
declares the real package selections for semantic classes. These registries
must stay exhaustive together. A missing producer, missing receipt, empty SBOM,
license denial, high/critical advisory, package mismatch, or provenance failure
is a failed main push, not a certification deferral.

## Live user tests

Strict v1 has one execution path. `apps/user-tests/scenarios.ts` is the typed
authority: one `USER_SCENARIOS` row per named scenario, each bound to one suite
(`packed`, `local`, `live`, `browser`, `money`, `operator`), and each suite's
`test/<suite>/registered.test.ts` runs exactly the rows registered to it. There
are no runtime-kind or capability matrices.

`apps/user-tests/artifacts.ts` resolves what a run exercises. Selection accepts
either an exact paired SDK/CLI version or a paired tarball/archive, never a
mixture.

Add a scenario by adding its row. Before making it a `live` row, check whether it
needs a remote plane at all: behavior that can be proved against the packed SDK
and a deterministic fixture belongs in the `packed` or `local` suite.

Contributor branch, review, and CI procedure lives in
[`contributing.md`](contributing.md). Repository-wide artifact cleanup follows
[`repository-hygiene.md`](repository-hygiene.md).
