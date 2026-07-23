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
   lint, type, and unit checks on its configured triggers.
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
| [`CI`](../.github/workflows/ci.yml) | Lint, type, and unit checks; main pushes publish the canary. |
| [`Live User Tests`](../.github/workflows/live-user-tests.yml) | Protected hosted API user tests and optional heavy canary. |

The public repository is the sole npm publisher. A green push to `main` tags
the tested source as `canary/<version>-canary`, binds the package to the exact
40-character source SHA in `aexRelease.sourceSha`, and publishes
`@aexhq/sdk@<version>-canary` to the `canary` dist-tag. The private platform
pipeline consumes that version and exact SHA for dev/prd validation.

The publish job runs only after lint, type, and unit checks pass. It uses npm
trusted-publisher OIDC, verifies registry visibility and source provenance, and
does not use a long-lived npm token. If the `<version>-canary` package version
already exists, the push fails; fix forward by bumping the base package version.
Workflow configuration is the exact source of truth and secret values must
never enter docs.

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
