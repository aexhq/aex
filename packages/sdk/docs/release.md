---
title: Release
---

# Release

The repository is Bun-first. Build, test, docs, package validation, and user
tests run through Bun.

npm publication is intentionally deferred after the Bun migration. The
`release.yml` and `promote.yml` workflows fail closed with a deferred-publish
message until package staging, provenance, and dist-tag promotion are
revalidated for the new toolchain.

## Current package gate

Before cutting a publish path, validate the package locally and in CI:

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

## Deferred publish path

When publication is reintroduced, the release workflow must explicitly solve
these items before it is enabled:

- Package staging for `workspace:*` dependencies.
- Provenance or a chosen replacement attestation path.
- Publish order for `@aexhq/contracts`, `@aexhq/conformance`, `@aexhq/cli`, and
  `@aexhq/sdk`.
- Post-publish user tests against the exact published version.
- Dist-tag promotion for canary-to-latest flows.

Do not enable a publish workflow by swapping in `bun publish` alone. The
workflow must prove that the published artifacts match the Bun-validated
tarballs and that clean Bun consumers can install and run them.

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

Until publication is reintroduced, rollback is a normal source revert. Once
npm publication returns, version numbers should be treated as immutable and bad
releases should be fixed by publishing a higher version.
