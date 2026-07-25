---
title: aex public repository agent guide
description: Table of contents for the public aex SDK, CLI, contracts, docs, and user-test repository.
keywords:
  - table of contents
  - public repository
  - SDK
  - CLI
  - references
audience: implementation agents and maintainers
status: accepted
related:
  - references/README.md
  - references/rules.md
  - references/repo.md
  - references/develop.md
  - references/repository-hygiene.md
---

# Agent guide

This repository owns the public aex product surface: SDK, CLI, contracts,
public docs, and user tests. This file is a table of contents only.
Put durable repository-wide rules, instructions, procedures, logs, and backlogs
under `references/`.

Before changing code, config, tests, or docs, read
[`references/README.md`](references/README.md),
[`references/rules.md`](references/rules.md), and the focused source below.

| Need | Canonical source |
| --- | --- |
| Reference index and authority map | [`references/README.md`](references/README.md) |
| Non-negotiable public-repository rules | [`references/rules.md`](references/rules.md) |
| Package and path ownership | [`references/repo.md`](references/repo.md) |
| Development, tests, and release routing | [`references/develop.md`](references/develop.md) |
| Contributor and review procedure | [`references/contributing.md`](references/contributing.md) |
| Durable-doc, diagnostics, scratch, and worktree placement | [`references/repository-hygiene.md`](references/repository-hygiene.md) |
| Public product orientation | [`README.md`](README.md) |
| Canonical SDK documentation | [`packages/sdk/docs/`](packages/sdk/docs/) |
| Public website source | [`apps/docs/content/docs/`](apps/docs/content/docs/) |
| Security reporting | [`SECURITY.md`](SECURITY.md) |

Commands live in `package.json`; user-visible contracts live with their owning
public package and tests.
