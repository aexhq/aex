# Contributing to aex

Thanks for wanting to help. This page is the contributor flow for the public
SDK, CLI, contracts, conformance helpers, user-test harness, and docs source.

## Before you open work

- **Open an issue first** for non-trivial changes so the design can
  be discussed before code is written. Small fixes and obvious bugs
  don't need this.
- Search existing issues and open PRs first to avoid duplication.
- Security issues go through [`SECURITY.md`](SECURITY.md) — do not
  file a public issue.

## Setup

```bash
bun install
```

Use the package scripts in `package.json` for build, lint, test, docs, and SDK
pack checks.

## Branch + PR flow

1. Fork and create a topic branch off `main`. Branch naming is
   informal — `fix/x`, `feat/x`, `docs/x` is fine.
2. Keep commits focused. Don't bundle unrelated changes into one PR.
3. Before pushing, run the relevant public-safe gates locally: `bun run lint`,
   `bun run test`, `bun run test:user:offline`, `bun run docs:build`, and
   `bun run pack:sdk`.
4. Open a PR against `main`. [`CI`](.github/workflows/ci.yml)
   runs the static/type/unit/offline user-test/docs/package gates after merge
   to `main` or manual dispatch.
5. Don't force-push `main`. Force-pushing your topic branch is fine.

## Commit messages

- Imperative mood, short subject (`fix: trim trailing slash on ...`).
- Conventional-commits prefixes (`feat:`, `fix:`, `chore:`,
  `refactor:`, `docs:`, `test:`, `ci:`) are encouraged but not
  enforced.
- **No AI-attribution trailers** — no `Generated with ...` and no
  `Co-Authored-By: <AI tool>`.

## What CI checks

| Workflow | Scope |
| --- | --- |
| [`CI`](.github/workflows/ci.yml) | main-push/manual lint, unit tests, offline user tests, docs build, and SDK pack/boundary check |
| [`Release`](.github/workflows/release.yml) | deferred npm publish placeholder; fails closed until the publish path is revalidated |
| [`Live User Tests`](.github/workflows/live-user-tests.yml) | manual protected hosted API user tests, with optional heavy canary |

Releases are manual and currently deferred. Re-enable the
[`Release`](.github/workflows/release.yml) workflow only after the Bun-first
package staging, provenance, and publish path has been revalidated.

## What reviewers look for

- A focused diff: one concern, no drive-by cleanups.
- Tests for new behaviour. Keep tests focused on the public SDK, CLI,
  contracts, conformance helpers, docs, or user-test harness affected by the
  change.
- No new public API surface without an entry in
  [`packages/sdk/docs/`](packages/sdk/docs/).
- No secrets, credentials, or `.env*` content in the diff.

## License

By contributing, you agree that your contributions are licensed under
the Apache License 2.0 (see [`LICENSE`](LICENSE)).
