---
title: Release
---

# Release

The public release path is intentionally small:

1. Local pre-push runs lint and type checks only.
2. A push to `main` runs lint, type checks, and unit tests.
3. After those checks pass, CI tags the tested source as `canary/<version>`,
   records the exact 40-character source SHA, and publishes
   `@aexhq/sdk@<version>` to npm's `canary` dist-tag.

The workflow checks out and verifies `github.sha`, writes that SHA into the
packed package's `aexRelease.sourceSha` metadata, and verifies the same value
against npm after publication.

The canary version is the base SDK version, the workflow run id, and the first
seven characters of the source SHA:

```
0.46.4-canary.19876543210.gb1c3d5f
```

The SHA is what makes the version unique per commit, so two pushes at one base
version resolve to two different versions and neither has to wait for the
other. The run id is what keeps the channel ordered, because semver compares
numeric prerelease identifiers numerically and a bare SHA suffix would sort
arbitrarily.

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
source and pushing again: the new commit resolves to a new canary version on
its own, and the base version only moves when the SDK's own semver does.
