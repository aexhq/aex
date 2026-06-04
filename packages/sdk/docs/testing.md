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
pnpm lint                                      # typecheck/build prerequisites + lint
pnpm test                                      # unit, all packages, deterministic
pnpm test:user:offline                         # clean install of packed/published SDK, no live API
pnpm test:user                                 # live hosted API user tests
pnpm test:user:heavy                           # explicit heavy live canary
pnpm run docs:build                            # generated docs + Next build
pnpm run pack:sdk                              # SDK publish dry-run + public boundary check
```

Unit tests are deterministic and may use fakes. Offline user tests install the
packed or published SDK into clean temp projects and do not need provider
credentials. Live user tests run against a hosted aex API and fail loudly
when required env is missing: `AEX_API_URL`, `AEX_API_TOKEN`,
`DEEPSEEK_API_KEY`, and exactly one of `AEX_USER_TEST_TARBALL` or
`AEX_USER_TEST_VERSION`.
