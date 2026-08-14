# Contributing to Aex

Thanks for wanting to help. This page is the whole contributor flow; GitHub
surfaces it in the pull-request UI, so nothing load-bearing lives behind a link.

## How work reaches `main`, and the asymmetry

Two paths exist, and they are not the same path:

- **Contributors open pull requests.** Fork, branch, push, open a PR against
  `main`. CI runs lint, type checks, and unit tests on every PR, including
  forked ones, and every required check must be green before merge.
- **The maintainer commits directly to `main`.** This is a pre-1.0 repository
  with one maintainer and a release train that reaches a hosted plane, so
  maintainer changes are not gated behind self-review.

That asymmetry is deliberate and stated here rather than discovered. It does not
change what your pull request is held to: the same gates, run by the same
workflow.

## Setup

```bash
bun install
```

Bun 1.3.14 or newer. Everything else is a package script:

| Command | What it covers |
| --- | --- |
| `bun run lint` | Brand, OpenAPI freshness, generated types, per-package lint, public-boundary check, ESLint. |
| `bun run typecheck` | Every package plus the repository scripts. |
| `bun run test:unit` | Package unit suites plus the repository validation suite. |

Those three are exactly what CI runs. Run them before pushing and there are no
surprises.

## Ground rules

- **Open an issue first** for anything non-trivial, so the design can be
  discussed before code exists. Obvious bug fixes do not need one.
- **One concern per pull request.** No drive-by cleanup inside a fix.
- **New behaviour needs a test** at the layer that owns it — the owning crate,
  the SDK, the CLI, or the repository validation suite.
- **No skipped tests.** The release gate asserts zero skipped or disabled
  entries; a test that cannot run is one to fix or delete, never to skip.
- **Public API changes need docs** under
  [`apps/site/content/docs/`](apps/site/content/docs/).
- **Never hand-edit generated output.** `api/generated/`, `crates/aex-wire/src/generated/`,
  `packages/wire/src/generated/` and `packages/sdk/src/generated/`
  are written by `cargo run -p aex-contract-gen -- build` from the schemas under
  `api/schemas/`, and the site reference tree is written by `apps/site/generate`.
  Edit the source and rerun the generator; a hand edit is reported by
  `cargo run -p aex-contract-gen -- check`.
- **No credentials, `.env` values, or unredacted diagnostics** in a diff.
- **No AI-attribution or AI co-author trailers** in commit messages.

Commit subjects are imperative mood, short, and conventionally prefixed
(`feat:`, `fix:`, `docs:`, `test:`, `chore:`, `refactor:`, `ci:`).

## What is in this repository, and what is not

Aex is open source under the Apache License 2.0. This repository holds the
public product surface — contracts, SDK, CLI, and docs — and, as the engine is
extracted, the agent execution runtime. The hosted control plane (accounts,
billing, scheduling, storage, and the infrastructure that runs them) is a
separate private repository.

Read [`references/architecture.md`](references/architecture.md) before proposing
a change that crosses that line. A pull request that assumes the control plane
lives here is a boundary question rather than a code-review question.

## Security

Do not open a public issue for a security bug. Use
[private vulnerability reporting](https://github.com/aexhq/aex/security/advisories/new);
the full policy is in [`SECURITY.md`](SECURITY.md).

## Releases

The public repository is the sole npm publisher. A green push to `main` tags the
exact tested source, binds each package to that 40-character source SHA, and
publishes a canary. Promotion of a canary to the stable dist-tag is always a
manual gate. The full procedure, including the workflow table, is in
[`references/contributing.md`](references/contributing.md).

## License

Contributions are licensed under the Apache License 2.0. See
[`LICENSE`](LICENSE) and [`NOTICE`](NOTICE). Opening a pull request confirms you
have the right to contribute the change under that license.
