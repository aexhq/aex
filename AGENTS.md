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
| System design and the open-core boundary | [`references/architecture.md`](references/architecture.md) |
| Vocabulary the source uses without defining | [`references/glossary.md`](references/glossary.md) |
| The `*.internal` runtime protocol | [`references/internal-protocol.md`](references/internal-protocol.md) |
| Public product orientation | [`README.md`](README.md) |
| Contributor entry point and the maintainer asymmetry | [`CONTRIBUTING.md`](CONTRIBUTING.md) |
| Community conduct | [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) |
| Canonical SDK documentation | [`packages/sdk/docs/`](packages/sdk/docs/) |
| Public website source | [`apps/docs/content/docs/`](apps/docs/content/docs/) |
| Security reporting | [`SECURITY.md`](SECURITY.md) |

Commands live in `package.json`; user-visible contracts live with their owning
public package and tests.

## Engineering canon (workspace root, not this repository)

The shared engineering canon, the standing engineering values, the testing layers
and locked rules, and the enforced house standards live **one directory up at the
workspace root**, where coding agents are spawned. Read them at these paths:

- `references/engineering.md` — the canon index and the standing values.
- `references/engineering/principles-code.md`, `principles-system.md`,
  `principles-operate.md`, `principles-trust.md`, `principles-laws.md` — the
  industry canon.
- `references/engineering/testing.md` — the testing canon, the five layers, and
  the locked rules. This repository's `apps/user-tests` is one of those layers,
  so the policy is shared rather than duplicated here.
- `references/code-standards.md` — the enforced house standards, including the
  canon/house-rules boundary and which one wins when they conflict.

Those paths are backticked prose, deliberately not markdown links. A relative
link out of this repository would dangle in a clone of this repository alone, and
this repository is public while the workspace root is not published — so a link is
the one form that could either break or leak. The workspace-root doc gate
(`bun test` in `scripts/docs`) validates markdown link targets and frontmatter
`related:` entries and never validates backticked prose, which is exactly what
keeps this pointer legal. Converting it to a link makes that gate fail.

Residual gap, stated plainly: a checkout of this repository on its own does not
contain the canon. That is accepted — agents are spawned at the workspace root,
and repository-only checkouts are CI runs, which execute tests rather than read
principle catalogs.
