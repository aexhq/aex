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
  - references/contributing.md
  - references/repository-hygiene.md
  - references/architecture.md
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
| Contributor and review procedure | [`contributing.md`](contributing.md) |
| Durable-doc and generated-artifact placement | [`repository-hygiene.md`](repository-hygiene.md) |
| Public v1 architecture and repository boundary | [`architecture.md`](architecture.md) |
| Rust-native rewrite implementation handoffs — what each stream landed, deferred and owes | [`rewrite/README.md`](rewrite/README.md) |
| Vocabulary the source uses without defining | [`glossary.md`](glossary.md) |

Future repository-wide designs, decision records, curated release logs, and
backlogs belong here. Do not create empty placeholders: add a focused,
frontmatter-indexed document only when durable content exists.

Root `README.md`, `CONTRIBUTING.md`, and `SECURITY.md` remain conventional
ecosystem entry points. Package READMEs, changelogs, public docs, and code-adjacent
documentation stay with their owning surface and link here for shared policy.
