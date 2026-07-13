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
| `packages/sdk/` | Public TypeScript SDK, canonical SDK docs, package changelog, and package tests. |
| `packages/cli/` | Public CLI commands, help, and CLI tests. |
| `packages/contracts/` | Public wire/runtime contracts and stable errors. |
| `packages/conformance/` | Public conformance helpers and assertions. |
| `apps/docs/` | Public documentation website and generated/reference presentation. |
| `apps/user-tests/` | Blackbox public SDK/CLI and published-artifact behavior. |
| `scripts/cicd/` | Public CI, package, release, and candidate-integrity tooling. |
| `scripts/validate/` | Repository and workflow policy checks. |
| `references/` | Repository-wide internal rules, procedures, decisions, logs, and backlog. |

`packages/sdk/docs/` is the canonical SDK prose source. Keep the public docs
site synchronized through its existing generator and validation rather than
hand-maintaining a second behavioral truth.

Root `README.md` is the public product landing page. `CONTRIBUTING.md` and
`SECURITY.md` are conventional entry points; canonical internal contributor
procedure lives in [contributing.md](contributing.md).
