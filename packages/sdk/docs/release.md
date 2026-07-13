---
title: Release
---

# Release

The repository is Bun-first. Build, test, docs, package validation, user tests,
and SDK packaging run through Bun. Registry publication uses npm on the
Bun-packed SDK tarball so npm provenance and dist-tag controls stay explicit.

## Package gate

Before publishing, validate the package locally and in CI:

```text
bun ci
bun run lint
bun run test
bun run test:user:offline
bun run docs:build
bun run pack:sdk
```

`bun run pack:sdk` builds the SDK, runs a Bun pack dry-run, and runs the public
boundary gate. Offline user tests install the packed SDK into a clean Bun
project and exercise the SDK and CLI from that install tree.

## Publish

Every push merged to `main` that passes the `CI` workflow automatically starts
`.github/workflows/release.yml`. The automatic path derives an immutable canary
version from the package version, release run, and source commit, then:

- Reuses the successful `main` CI result instead of rerunning the same static,
  unit, offline-user-test, docs, and package gates.
- Packs `packages/sdk` with `bun pm pack`.
- Binds the tarball manifest to the exact 40-character release commit under
  `aexRelease.sourceSha`.
- Publishes the tarball through npm trusted-publisher OIDC to the `canary`
  dist-tag, without a long-lived npm token.
- Waits for npm visibility.
- Runs the focused published SDK/CLI smoke against that exact version.
- Records the source commit, npm integrity, and release-run identity in a public
  release manifest before the immutable candidate enters validation.

The immutable candidate must pass the required validation before promotion.
Neither the public smoke nor the validation suite retries a failed user
scenario.

The canary version is deterministic for a release run. Restarting an interrupted
automatic release resumes the same already-published version after verifying its
identity; it never overwrites an npm artifact.

`release.yml` also supports manual dispatch to the `canary` or `next` lane. A
manual release reruns all package gates because it has no upstream successful
`main` CI run to trust, and it refuses to publish an existing package version.
Neither path may publish directly to `latest`.

Each merge therefore produces a traceable prerelease. Promote that same
immutable version only after its release gate is green.

## npm credentials

`release.yml` publishes through npm trusted-publisher OIDC for the
`aexhq/aex` repository and `release.yml` workflow. The publish job does not use
a long-lived npm write token. npm dist-tag changes use separate, narrowly scoped
authentication because trusted publishing does not authorize `npm dist-tag`.

## Promote

`.github/workflows/promote.yml` assigns an npm dist-tag to an already-published
`@aexhq/sdk` version, normally `latest` after the exact canary passes downstream
validation. Promotion is manual and fail-closed: the workflow verifies the
candidate version, release identity, validation proof, source commit, npm
integrity, and target dist-tag before moving the tag. It also requires the source commits
currently attested by both `latest` and `canary` to be ancestors of the
candidate, so a higher version cannot move a tag to older or divergent code.
Versions before `0.42.0` predate the registry source field and are the only
explicit migration exception; missing source proof fails closed from `0.42.0`
onward. Promotion never rebuilds or republishes the package.

## What ships in the tarball

The SDK tarball is self-contained. It declares zero `@aexhq/*` runtime
dependencies and is installable from a clean Bun project with no workspace
access:

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
  `apps/user-tests/test/offline/install.test.ts` before publish.

## Rollback

Published version numbers are immutable. Bad releases are fixed by publishing a
higher version and moving dist-tags forward; source rollback alone does not
remove an npm artifact.
