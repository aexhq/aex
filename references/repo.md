---
title: public repository structure
description: Path and source-of-truth ownership for the public aex repository.
keywords:
  - repository
  - ownership
  - SDK
  - CLI
  - contracts
  - docs
  - user tests
audience: implementation agents and maintainers
status: accepted
related:
  - references/rules.md
  - references/develop.md
---

# Public repository structure

| Path | Owns |
| --- | --- |
| `api/schemas/` | Authored strict wire schemas, identifiers, routes, errors, scopes, and limits. The contract source. |
| `api/generated/`, `api/openapi/` | Generated bundle, OpenAPI documents, and per-schema JSON Schema. Never hand-edited. |
| `crates/` | Domain, application, and adapter crates, including the generated `aex-wire` contract. |
| `services/`, `runtimes/`, `workers/` | Deployable composition roots. |
| `tools/` | Repository tooling: `aex-cli`, `aex-contract-gen`, `aex-release-tool`, `aex-workspace-check`. |
| `packages/sdk/` | Public TypeScript SDK, package changelog, and package tests. |
| `packages/wire/` | Generated TypeScript wire models, strict validators, and identifier helpers. `aex-contract-gen` owns `src/generated/`. |
| `apps/site/` | Public marketing and documentation website, its deterministic generator, and the shared `apps/site/design/` token system. |
| `apps/dashboard/` | Stateless dashboard and its BFF. It consumes the shared site tokens and component primitives rather than defining a second palette. |
| `apps/user-tests/` | Blackbox public SDK/CLI and published-artifact behavior. |
| `migrations/` | Central SQL migrations and the regional table generation definitions. |
| `infra/` | Terraform modules and composition examples. |
| `release/` | Deployable units, path map, scenario ownership, policy, and derived evidence registries. |
| `conformance/` | The generated conformance corpus. |
| `tests/` | Cross-crate live companions, load workloads, and shared test support. |
| `scripts/cicd/` | npm packaging, legal-file sync, and validation-suite plumbing. |
| `scripts/validate/` | Repository and workflow policy checks. |
| `references/` | Repository-wide internal rules, procedures, decisions, logs, and backlog. |
| `.github/` | Workflows, issue and pull-request templates, `CODEOWNERS`, and dependency automation. |

`apps/site/content/docs/` is the canonical public prose source. Keep it
generated from the contract rather than hand-maintaining a second behavioral
truth.

`apps/site/content/marketing/index.mdx` is the single source for every claim on
the landing page. Components under `apps/site/app/` own layout and carry no
copy, so changing a claim is a one-file edit. Every claim there must be
traceable to an accepted design record; the page deliberately names the four
usage meters without publishing a rate, because
[rules.md](rules.md) keeps billing/rate policy out of public docs and the
accepted Area 5 `U-COGS` decision keeps every real rate-book revision out of
this repository.

`apps/site/design/` is the shared design system: `tokens.css` (colour, type
scale, spacing, radius, elevation, motion), `base.css` (bare-element styling),
`components.css` (container, button, card, note, badge, code, skip link), and
`index.css`, which is the only file a consumer imports. Both light and dark are
defined, selected by `prefers-color-scheme` or an explicit `data-theme`
attribute. `apps/site/test/design-tokens.test.ts` asserts AA contrast for every
composed pair in both themes, so a colour change is checked rather than
reviewed.

Root `README.md` is the public product landing page. `CONTRIBUTING.md`,
`SECURITY.md`, and `CODE_OF_CONDUCT.md` are conventional entry points, and
`CONTRIBUTING.md` is self-contained because GitHub surfaces it directly in the
pull-request UI; the longer internal procedure stays in
[contributing.md](contributing.md).

## Legal files are generated, not hand-maintained

Root `LICENSE` and `NOTICE` are the single source. Every publishable package
carries a byte-identical copy so its npm tarball is a lawful redistribution —
a tarball cannot reach up to a repository root that is not inside it.

| Command | Effect |
| --- | --- |
| `bun scripts/cicd/sync-package-legal.ts` | Copies root `LICENSE` and `NOTICE` into every publishable package. |
| `bun scripts/cicd/sync-package-legal.ts --check` | Fails on drift. Also asserted by `scripts/validate/package-metadata.test.ts`. |
| `bun scripts/cicd/public-modules.ts` | Prints the derived public module registry. |

"Publishable" is derived, never listed: a workspace member is a public module
exactly when its manifest does not say `private: true`. That is the same field
npm obeys, so the registry and the publisher cannot disagree.
