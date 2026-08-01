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
| `apps/site/` | Public marketing and documentation website, and its deterministic generator. |
| `apps/dashboard/` | Stateless dashboard and its BFF. |
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
