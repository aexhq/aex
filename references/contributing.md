---
title: contributing to aex
description: Canonical contributor branch, review, test, CI, and public-release procedure.
keywords:
  - contributing
  - pull request
  - CI
  - review
  - release
audience: contributors and maintainers
status: accepted
related:
  - references/rules.md
  - references/develop.md
---

# Contributing to aex

Thanks for wanting to help. This is the canonical contributor flow for the
public SDK, CLI, contracts, conformance helpers, user-test harness, and docs
source.

## Before you open work

- **Open an issue first** for non-trivial changes so the design can be discussed
  before code is written. Small fixes and obvious bugs do not need this.
- Search existing issues and open pull requests first to avoid duplication.
- Security issues go through [`SECURITY.md`](../SECURITY.md); do not file a
  public issue.

## Setup

```bash
bun install
```

Use the package scripts in `package.json` for build, lint, test, docs, and SDK
pack checks.

## Branch and pull-request flow

1. Fork and create a topic branch off `main`. Informal names such as `fix/x`,
   `feat/x`, or `docs/x` are fine.
2. Keep commits focused; do not bundle unrelated changes.
3. Before pushing, run the relevant public-safe gates locally from
   `package.json`.
4. Open a pull request against `main`. [CI](../.github/workflows/ci.yml) runs
   static, type, unit, offline user-test, docs, and package gates on its
   configured triggers.
5. Do not force-push `main`. Force-pushing a topic branch is acceptable only
   when it does not disrupt another contributor.

## Commit messages

- Use imperative mood and a short subject.
- Conventional prefixes such as `feat:`, `fix:`, `chore:`, `refactor:`,
  `docs:`, `test:`, and `ci:` are encouraged.
- Do not add AI-attribution or AI co-author trailers.

## CI and release ownership

| Workflow | Scope |
| --- | --- |
| [`CI`](../.github/workflows/ci.yml) | Static/type/unit/offline user-test/docs/package gates. |
| [`Release`](../.github/workflows/release.yml) | Immutable canary publication after eligible green main CI, plus published-artifact smoke. |
| [`Live User Tests`](../.github/workflows/live-user-tests.yml) | Protected hosted API user tests and optional heavy canary. |

The public repository is the sole npm publisher. An exact canary is validated
against the hosted service and becomes eligible for explicit promotion only
after its downstream evidence gate passes. The public repository must not
encode private deployment internals.

Both public publish and promotion workflows run in the `npm-release` GitHub
Environment. `release.yml` publishes through npm trusted-publisher OIDC and
must not receive a write token. `promote.yml` still needs the Environment's
narrowly scoped `NPM_TOKEN` because npm OIDC does not authorize `npm dist-tag`
operations. Workflow configuration is the exact source of truth and secret
values must never enter docs.

## Review criteria

- One focused concern and no drive-by cleanup.
- Tests for new behavior at the public SDK, CLI, contracts, conformance, docs,
  or user-test layer that owns it.
- Public API changes documented under [`packages/sdk/docs/`](../packages/sdk/docs/).
- No credentials, `.env*` values, private hosted detail, or unredacted
  diagnostics in the diff.

## License

Contributions are licensed under the Apache License 2.0; see
[`LICENSE`](../LICENSE).
