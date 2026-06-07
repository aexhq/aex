---
title: Release
---

# Release

Releases are manually dispatched from `.github/workflows/release.yml` after the
target version is already on `main`. The npm package version is the release
source of truth. The workflow publishes to npm and runs post-publish install
checks, but it does not create git tags or GitHub Releases, keeping the remote
repository on a clean `main` branch unless tags are added deliberately later.

## How to ship a release

1. Bump the publishable package manifests to the next shared semver:
   `packages/contracts/package.json`, `packages/conformance/package.json`,
   `packages/cli/package.json`, and `packages/sdk/package.json`. Also update
   `packages/sdk/src/version.ts` to match.
2. Land the change on `main` with any companion code or docs.
3. Confirm CI is green.
4. Run the **Release** workflow from `main` and choose the npm dist-tag
   (`latest` or `next`).

If any package at `<version>` already exists on npm, the release workflow fails
before publishing. A failed release is fixed by bumping to a higher version and
running the workflow again.

## Release pipeline

The workflow has two jobs:

1. **`publish`** runs on Ubuntu in the protected `npm-release` environment:
   - `pnpm install --frozen-lockfile`
   - npm version availability checks for `@aexhq/contracts`,
     `@aexhq/conformance`, `@aexhq/cli`, and `@aexhq/sdk` at
     `packages/sdk/package.json#version`
   - `pnpm lint`
   - `pnpm test`
   - `pnpm run docs:build`
   - `pnpm build`
   - `pnpm --filter @aexhq/sdk pack`
   - `pnpm run test:user:offline` against the packed tarball
   - a final npm version availability check
   - `pnpm publish --provenance --no-git-checks --access public` for
     `@aexhq/contracts`, `@aexhq/conformance`, `@aexhq/cli`, and `@aexhq/sdk`
2. **`post-publish-user-tests`** waits for npm registry visibility, then runs
   `pnpm run test:user:offline` against the published version on Ubuntu and
   Windows.

The pre-publish user-test gate catches broken package shape, missing CLI bin,
workspace dependency leaks, and TypeScript/ESM install regressions before npm
publish. The post-publish matrix confirms the published registry artifact is
installable from clean user projects on both Linux and Windows.

## What ships in the tarball

The published tarball is **self-contained**. It declares **zero `@aexhq/*`
runtime dependencies** and is installable from a clean `npm install @aexhq/sdk`
with no workspace access:

- `@aexhq/contracts` lives in `packages/sdk/package.json#devDependencies`
  only. At build time, [`packages/sdk/scripts/inline-contracts.mjs`](../scripts/inline-contracts.mjs)
  copies `packages/contracts/dist/**` into `packages/sdk/dist/_contracts/` and
  rewrites `from "@aexhq/contracts"` to `from "./_contracts/index.js"` across
  the SDK dist tree. A sanity check at the end of that script refuses to finish
  if any bare `@aexhq/contracts` specifier survives.
- `@aexhq/cli` is bundled at build time by
  [`packages/sdk/scripts/bundle-cli.mjs`](../scripts/bundle-cli.mjs) into a
  single `dist/cli.mjs`, which is the `bin: aex` entry in
  `packages/sdk/package.json`.
- This invariant is mechanically enforced by
  `apps/user-tests/test/offline/install.test.ts` ("declares no @aexhq/*
  runtime dependencies") before publish.

## Repository setup

Set `NPM_TOKEN` in the protected GitHub `npm-release` environment. The token must
have publish rights for all four packages. The workflow still publishes with
provenance.

Trusted Publishing can replace the token after every package exists on npm and
the trusted publisher is configured for each package:

- **Organization or user**: `aexhq`
- **Repository**: `aex`
- **Workflow filename**: `release.yml`
- **Environment name**: `npm-release`

## Local checklist

Before dispatching a release, the same public-safe checks can be run locally:

```text
pnpm lint
pnpm test
pnpm run test:user:offline
pnpm run docs:build
pnpm run pack:sdk
```

## Rollback

There is no reliable "unpublish and reuse the version" path. npm version numbers
are effectively immutable for release purposes, so a bad release is fixed by
publishing a higher version.
