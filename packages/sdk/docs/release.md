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
- Runs the publish job in the `npm-release` GitHub Environment and requires its
  `NPM_TOKEN` secret.
- Publishes the tarball with npm provenance to the `canary` dist-tag.
- Waits for npm visibility.
- Runs live user tests against that exact published version.
- Records the source commit, npm integrity, and release-run identity in a public
  release manifest before dispatching the immutable candidate for downstream
  validation.

The canary version is deterministic for a release run. Restarting an interrupted
automatic release resumes the same already-published version after verifying its
identity; it never overwrites an npm artifact.

`release.yml` also supports manual dispatch to the `canary` or `next` lane. A
manual release reruns all package gates because it has no upstream successful
`main` CI run to trust, and it refuses to publish an existing package version.
Neither path may publish directly to `latest`.

Each merge therefore produces a traceable prerelease. Promote that same
immutable version only after the downstream release gate is green.

## npm credentials

The current release path uses a GitHub environment secret:

- Environment: `npm-release`
- Secret name: `NPM_TOKEN`

The workflow intentionally fails before `npm publish` if that token is absent.
Do not remove the `environment: npm-release` binding unless the package has been
migrated to npm trusted publishing.

Trusted publishing is the target credential model, but it is a separate npm
package setting, not just a workflow permission. Before removing `NPM_TOKEN`,
configure the npm trusted publisher for `aexhq/aex`, workflow file
`release.yml`, and environment `npm-release`, then run the workflow with an npm
CLI version that supports OIDC trusted publishing.

## Promote

`.github/workflows/promote.yml` assigns an npm dist-tag to an already-published
`@aexhq/sdk` version, normally `latest` after the exact canary passes downstream
validation. Promotion is manual and fail-closed: an operator dispatches the
workflow with the required version, release-run id, downstream validation-run
id, source commit, npm integrity, and target dist-tag. The workflow verifies
that complete proof before moving the tag. It runs in `npm-release`, requires
`NPM_TOKEN`, and never rebuilds or republishes the package.

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
