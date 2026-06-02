---
title: Release
---

# Release

Releasing is **atomic** and **driven by `packages/sdk/package.json#version`**: bump the SDK version on `main` and the publish pipeline carries that exact version through npm → git tag → GitHub Release in one job. Any push to `main` that does not bump the version is a publish no-op.

## How to ship a release

1. On a branch, bump `packages/sdk/package.json#version` to the next semver. Land any companion code/doc changes in the same PR.
2. Merge into `main` (or push directly if you have the right). The pre-push hook (see [Local guard rails](#local-guard-rails)) refuses the push if the local version still matches what's on npm or has already been git-tagged.
3. CI (`.github/workflows/ci-fast.yml`) runs the `version-gate` job on every PR and direct push to `main`. It diffs against the base ref using [`scripts/check-version-drift.mjs --strict`](../../../scripts/check-version-drift.mjs) and fails fast if anything under `packages/{sdk,cli,shared}/src/` or the listed exact paths changed without a fresh version. **`--strict` treats unreachable npm as a hard failure** so we never publish on top of an existing version.
4. After merge, the `Publish package` workflow (`.github/workflows/publish.yml`) is triggered automatically on push to `main` and decides whether to ship.

You don't tag locally and you don't open a GitHub Release manually — the workflow does both. If the workflow ends red after a successful `npm publish`, the npm version is the source of truth and a human can tag/release retroactively.

## Publish pipeline

The workflow has three jobs, in this order:

1. **`decide`** — compares the local `packages/sdk/package.json#version` with `npm view antpath@latest version`.
   - If they match → output `proceed=false` and the rest of the pipeline no-ops. Documentation-only commits, refactors that ride along with a previous version, and any other non-publishable change pass through cleanly.
   - If they differ → `proceed=true`, with the local version flowing forward as the canonical `v<version>` for tagging and release notes.
2. **`publish`** (gated on `proceed=true`) — single linear job:
   - `pnpm install --frozen-lockfile`, `pnpm lint`, `pnpm test`, `pnpm build`.
   - `pnpm --filter antpath pack` into `$RUNNER_TEMP`.
   - **Pre-publish user-tests gate**: runs `apps/user-tests` `test:user:offline` against the packed tarball. A broken artifact (missing `bin`, broken shebang, leaked workspace dep, ESM-only contract violation, …) never reaches npm.
   - **Final freshness check**: `npm view antpath@${VERSION} version` — guards the rare race where someone else publishes the same version between `decide` and here.
   - `pnpm publish --provenance --no-git-checks` from `packages/sdk`. Auth is GitHub OIDC via `id-token: write` plus npm Trusted Publishers — there is no `NPM_TOKEN` secret.
   - `git tag -a v${VERSION}` signed as the canonical commit identity, then `git push origin v${VERSION}`.
   - `gh release create v${VERSION} --generate-notes`.
3. **`user-tests-post-publish`** (matrix `ubuntu-latest, windows-latest`) — waits for registry visibility with `scripts/wait-for-npm.mjs antpath <version>` (catches the "metadata says yes but CDN tarball 404s" case), then runs `test:user:offline` against the published version on both runners.

Atomicity boundary: **publish + tag + GitHub Release happen in the same job**. Until that job goes green, the release is not done — even if `npm publish` already succeeded. Post-publish user-tests are a follow-on signal, not part of the atomic act.

## What ships in the tarball

The published tarball is **self-contained**. It declares **zero `@antpath/*` runtime dependencies** and is installable from a clean `npm install antpath` with no workspace access:

- `@antpath/contracts` lives in `packages/sdk/package.json#devDependencies` only. At build time, [`packages/sdk/scripts/inline-contracts.mjs`](../scripts/inline-contracts.mjs) copies `packages/contracts/dist/**` into `packages/sdk/dist/_contracts/` and rewrites `from "@antpath/contracts"` to `from "./_contracts/index.js"` across the SDK dist tree. A sanity check at the end of that script refuses to finish if any bare `@antpath/contracts` specifier survives.
- `@antpath/cli` is bundled at build time by [`packages/sdk/scripts/bundle-cli.mjs`](../scripts/bundle-cli.mjs) into a single `dist/cli.mjs`, which is the `bin: antpath` entry in `packages/sdk/package.json`.
- This invariant is mechanically enforced by `apps/user-tests/test/offline/install.test.ts` ("declares no @antpath/* runtime dependencies") — the pre-publish gate fails before npm if any workspace dep leaks back into `dependencies`/`peerDependencies`/`optionalDependencies`.

## Local guard rails

Every contributor who pushes is expected to install the tracked pre-push hook from the repo root once:

```text
pnpm hooks:install
```

The hook runs lint, tests, build, an SDK pack dry-run, and `check-version-drift.mjs` (advisory mode — unreachable npm is a warning so offline development still works). It catches "I bumped a runtime file but forgot to bump the version" or "the new version is already on npm" before the push leaves the laptop.

## Repository setup (one-time)

1. Reserve `antpath` on npm.
2. Add a Trusted Publisher for this repository (`npmjs.com` → *Settings* → *Publishing access*):
   - **Organization or user**: `weilueluo`
   - **Repository**: `antpath`
   - **Workflow filename**: `publish.yml`
   - **Environment name**: leave empty.
3. No `NPM_TOKEN` secret is required — OIDC handles auth.

## Rollback

There is no "unpublish" path: npm prevents reuse of a published version, and a bad release is fixed by publishing a higher version. If the atomic publish job dies between `npm publish` and `gh release create`, retag manually (`git tag -a v<version>`, push, `gh release create`); npm is already truthful.
