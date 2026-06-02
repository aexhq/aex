# Contributing to antpath

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
pnpm install
```

Use the package scripts in `package.json` for build, lint, test, docs, and SDK
pack checks.

## Branch + PR flow

1. Fork and create a topic branch off `main`. Branch naming is
   informal — `fix/x`, `feat/x`, `docs/x` is fine.
2. Keep commits focused. Don't bundle unrelated changes into one PR.
3. Before pushing, the `pre-push` hook runs `pnpm lint`, `pnpm test`,
   `pnpm build`, `pnpm pack:sdk`, and a version-drift check. If you
   bypass the hook locally, CI will run the same checks.
4. Open a PR against `main`. [`Checks`](.github/workflows/ci.yml)
   runs the zero-cost static/type/build/unit gates automatically.
5. Don't force-push `main`. Force-pushing your topic branch is fine.

## Commit messages

- Imperative mood, short subject (`fix: trim trailing slash on ...`).
- Conventional-commits prefixes (`feat:`, `fix:`, `chore:`,
  `refactor:`, `docs:`, `test:`, `ci:`) are encouraged but not
  enforced.
- **No AI-attribution trailers** — no `Generated with ...`, no
  `Co-Authored-By: <AI tool>`. See
  [`CONTRIBUTING.md`](CONTRIBUTING.md#identity).

## What CI checks

| Workflow                | Scope                                                  |
| ----------------------- | ------------------------------------------------------ |
| `Checks`                | automatic zero-cost lint/type/build/unit gates         |
| `CLI` / `SDK`           | CLI validation and SDK package publish path            |
| `Docs`                  | documentation source generation and type checks        |

Releases are manual. SDK releases are driven by bumping
`packages/sdk/package.json#version`, then running the
[`SDK`](.github/workflows/sdk.yml) workflow; the
[`Publish package`](.github/workflows/publish.yml) workflow publishes to npm and
cuts a GitHub Release only when called by the SDK release path.

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
