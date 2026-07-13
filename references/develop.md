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

Contributor branch, review, and CI procedure lives in
[`contributing.md`](contributing.md). Repository-wide artifact cleanup follows
[`repository-hygiene.md`](repository-hygiene.md).
