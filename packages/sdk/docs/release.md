---
title: Release
---

# Release

Each publishable package has its own prelaunch canary version and immutable
source tag. A main-push release records:

- the exact 40-character source SHA;
- the package version and npm integrity;
- exact same-run upstream package versions.

The SDK, CLI, and contracts are independently versioned. The CLI bundle is
standalone at runtime; its SDK and contracts build inputs remain part of the
recorded release identity.

Publication uses npm trusted-publisher OIDC and the `canary` dist-tag. This
prelaunch repository does not publish `1.0.0` or promote a canary to `latest`.
Published artifacts are immutable; fixes roll forward from a new commit.
