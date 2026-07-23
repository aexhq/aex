---
title: Release
---

# Release

The public release path is intentionally small:

1. Local pre-push runs lint and type checks only.
2. A push to `main` runs lint, type checks, and unit tests.
3. After those checks pass, CI tags the tested source as
   `canary/<version>-canary`, records the exact 40-character source SHA, and
   publishes `@aexhq/sdk@<version>-canary` to npm's `canary` dist-tag.

The workflow checks out and verifies `github.sha`, writes that SHA into the
packed package's `aexRelease.sourceSha` metadata, and verifies the same value
against npm after publication. The package version is the base SDK version with
`-canary` appended, for example `0.45.0-canary`. A version is immutable: if it
already exists, fix forward by bumping the base SDK version and pushing again.

The private platform repository receives the canary version and source SHA as
manual deploy inputs. It owns dev/prd deployment and runs the same SDK-as-a-real-
user suite against both Lambda and container runtimes in each plane.

## npm credentials

The main-push publish job uses npm trusted-publisher OIDC in the `npm-release`
GitHub Environment. It does not use a long-lived npm write token and never
publishes directly to `latest`.

## What ships in the tarball

The SDK tarball is self-contained. It declares zero `@aexhq/*` runtime
dependencies and is installable from a clean Bun project with no workspace
access:

- `@aexhq/contracts` is copied into the SDK distribution at build time.
- `@aexhq/cli` is bundled into `dist/cli.mjs`, exposed through the `aex` bin.
- The offline user-test package checks these invariants before publication.

## Roll-forward

Published versions are immutable. A bad canary is fixed by correcting the
source, bumping the base version, and publishing a new canary.
