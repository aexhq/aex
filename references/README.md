---
title: public repository references index
description: Entry point for internal public-repository rules, ownership, development, contributor procedure, and repository hygiene.
keywords:
  - references
  - index
  - public repository
  - internal documentation
audience: implementation agents and maintainers
status: accepted
related:
  - references/rules.md
  - references/repo.md
  - references/develop.md
  - references/backlog.md
  - references/release-evidence.md
  - references/npm-trusted-publisher-bootstrap.md
  - references/contributing.md
  - references/repository-hygiene.md
  - references/architecture.md
  - references/account-pause-control.md
  - references/glossary.md
---

# Public repository references

This directory owns durable repository-wide internal guidance. `AGENTS.md` is
only the navigation entry point. Public user documentation lives in
`apps/site/content/docs/`; commands and configuration remain in their
executable sources.

| Need | Source |
| --- | --- |
| Non-negotiable rules and public boundary | [`rules.md`](rules.md) |
| Package and path ownership | [`repo.md`](repo.md) |
| Development, testing, and release routing | [`develop.md`](develop.md) |
| Deferred work and revisit triggers | [`backlog.md`](backlog.md) |
| One-time public GHCR namespace bootstrap | [`ghcr-visibility-bootstrap.md`](ghcr-visibility-bootstrap.md) |
| One-time npm trusted publisher bootstrap for the SDK | [`npm-trusted-publisher-bootstrap.md`](npm-trusted-publisher-bootstrap.md) |
| Model-provider transport and catalog simplification (2026-08-12) — proposal, superseded in part; see the follow-up folder | [`model-provider-library-simplification-2026-08-12.md`](model-provider-library-simplification-2026-08-12.md) |
| Model-provider library simplification — owner decisions, spikes, implementation plans (2026-08-13) — **implemented 2026-08-13** | [`model-provider-library-simplification-2026-08-13/README.md`](model-provider-library-simplification-2026-08-13/README.md) |
| Release-bound E2E and public user receipt producer | [`release-evidence.md`](release-evidence.md) |
| Contributor and review procedure | [`contributing.md`](contributing.md) |
| Durable-doc and generated-artifact placement | [`repository-hygiene.md`](repository-hygiene.md) |
| Public v1 architecture and repository boundary | [`architecture.md`](architecture.md) |
| Account-pause admission fence, active-session interruption, and consistency model | [`account-pause-control.md`](account-pause-control.md) |
| Generated TypeScript wire binding and consumer migration boundary | [`typescript-wire-binding.md`](typescript-wire-binding.md) |
| Rust-native rewrite implementation handoffs — what each stream landed, deferred and owes | [`rewrite/README.md`](rewrite/README.md) |
| Collapsing the central HTTP surface onto one Fargate service (2026-08-09) — accepted | [`central-api-fargate-consolidation-2026-08-09.md`](central-api-fargate-consolidation-2026-08-09.md) |
| Where tool work executes, and how attribution survives leaving the Brain (2026-08-09) — accepted | [`tool-execution-placement-2026-08-09.md`](tool-execution-placement-2026-08-09.md) |
| Identifying a sandbox to the platform for platform-paid tool calls (2026-08-09) — proposal | [`sandbox-platform-identity-2026-08-09.md`](sandbox-platform-identity-2026-08-09.md) |
| Vocabulary the source uses without defining | [`glossary.md`](glossary.md) |

Future repository-wide designs, decision records, curated release logs, and
backlogs belong here. Do not create empty placeholders: add a focused,
frontmatter-indexed document only when durable content exists.

Root `README.md`, `CONTRIBUTING.md`, and `SECURITY.md` remain conventional
ecosystem entry points. Package READMEs, changelogs, public docs, and code-adjacent
documentation stay with their owning surface and link here for shared policy.
