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
public SDK, CLI, contracts, user-test harness, and docs
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
4. Open a pull request against `main`. [`pr`](../.github/workflows/pr.yml) runs
   the repository gates and the routed Rust, TypeScript, Terraform, and artifact
   lanes; its `checks` job is the single required status.
5. Do not force-push `main`. Force-pushing a topic branch is acceptable only
   when it does not disrupt another contributor.

## Commit messages

- Use imperative mood and a short subject.
- Conventional prefixes such as `feat:`, `fix:`, `chore:`, `refactor:`,
  `docs:`, `test:`, and `ci:` are encouraged.
- Do not add AI-attribution or AI co-author trailers.

## CI and release ownership

There are four lane classes. Every other workflow file is a reusable lane one of
them calls, and each lane ends in a receipts job that compares what the lane
declared it would run against the receipts it actually produced.

| Lane | Scope |
| --- | --- |
| [`pr`](../.github/workflows/pr.yml) | Pull request and merge queue. Repository gates plus the routed Rust, TypeScript, Terraform, and artifact lanes. No cloud, registry, publish, or signing credential reaches it. |
| [`main`](../.github/workflows/main.yml) | Protected `main` build and publication. Mints immutable bytes and a composition manifest; applies nothing to any plane. |
| [`assurance`](../.github/workflows/assurance.yml) | Scheduled full-graph, supply-chain, deep-risk, cold-rebuild, and plane suites whose receipts a release admits against. |
| [`release`](../.github/workflows/release.yml) | Explicit environment release, dispatched with an exact manifest digest. Never a branch, tag, run, or floating pointer. |

Publication is not deployment. `main` produces artifacts and a manifest;
`release` is the only lane that touches a plane, and it is manual, per-plane
serialized, and refuses anything but a pinned composition digest. Workflow
configuration is the exact source of truth and secret values must never enter
docs.

## Review criteria

- One focused concern and no drive-by cleanup.
- Tests for new behavior at the crate, SDK, CLI, site, or user-test layer that
  owns it.
- Public API changes documented under [`apps/site/content/docs/`](../apps/site/content/docs/).
- No credentials, `.env*` values, private hosted detail, or unredacted
  diagnostics in the diff.

## License

Contributions are licensed under the Apache License 2.0; see
[`LICENSE`](../LICENSE).
