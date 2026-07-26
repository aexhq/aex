---
title: Testing
---

# Testing

aex uses test-first development.
For repository changes, default the first meaningful test to blackbox behavior
from requirements, public contracts, docs, logs, or user-visible evidence before
reading or changing implementation.

Workspace-wide commands:

```text
bun run lint                                      # typecheck/build prerequisites + lint
bun run test                                      # unit, all packages, deterministic
bun run test:user:offline                         # clean Bun install of packed/published SDK, no live API
bun run test:user                                 # live hosted API user tests
bun run test:user:heavy                           # explicit heavy live canary
bun run docs:build                                # generated docs + Next build
bun run pack:sdk                                  # SDK pack dry-run + public boundary check
```

Unit tests are deterministic and may use fakes. Offline user tests install the
packed or published SDK into clean Bun temp projects and do not need provider
credentials.

When neither `AEX_USER_TEST_TARBALL` nor `AEX_USER_TEST_VERSION` is set, the
user-test fixture packs the current workspace SDK into a tempdir and installs
that artifact. CI or release validation can pin an explicit artifact with
exactly one of `AEX_USER_TEST_TARBALL` or `AEX_USER_TEST_VERSION`.

Live user tests run against a hosted aex API and fail loudly when required env
is missing: `AEX_API_URL` and `AEX_API_KEY`. Model access uses the managed
gateway; no customer provider key is part of test configuration.
