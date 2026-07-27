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

## Live user tests

Every file under `apps/user-tests/test/live/` must declare a coverage tier in
`apps/user-tests/scripts/live-coverage.mjs`. A new live file with no entry fails
the sweep, both CI matrix builders, and lint — there is no default tier.

| Tier | Runs on | Pick it when |
| --- | --- | --- |
| `runtime-spotcheck` | every full-coverage arm | The file is one of the two per-entry-point parity anchors (one CLI, one SDK) and emits parity verdict cells. |
| `runtime-matrix` | every full-coverage arm (`lambda` + `spot_container` on dev) | The execution runtime decides the outcome: workspace materialization, in-runtime tool execution, journal/park semantics, capacity. |
| `runtime-agnostic` | one arm, duration-packed into shared bins | The control plane, the SDK client, or the API boundary decides the outcome and the runtime is incidental. |
| `on-demand` | never in the default live sweep; an explicit dedicated workflow lane | Provider-family, cap-saturating, load-shaped, or otherwise probabilistic coverage. |

Default to `runtime-agnostic` and justify anything higher — each promotion to
`runtime-matrix` multiplies by the number of arms. Before adding a live file,
check whether the assertion needs a plane at all: a case that injects a fetch, or
asserts no HTTP request was made, belongs in `test/offline/`.

Contributor branch, review, and CI procedure lives in
[`contributing.md`](contributing.md). Repository-wide artifact cleanup follows
[`repository-hygiene.md`](repository-hygiene.md).
