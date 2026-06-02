---
title: Testing
---

# Testing

antpath uses test-first development.
For repository changes, default the first meaningful test to blackbox behavior
from requirements, public contracts, docs, logs, or user-visible evidence before
reading or changing implementation. See the repository testing policy at
`../../docs/testing.md`.

Workspace-wide commands:

```text
pnpm test                                       # unit, all packages, deterministic
pnpm test:integration                           # live external systems — no skip flags
pnpm test:e2e                                   # full top-to-bottom flows against live services
pnpm test:user                                  # published antpath package (offline + live)
```

Unit tests are deterministic and may use fakes. Integration tests run live external systems without any skip flag; if credentials are missing they fail loudly. Live e2e and user-live tests require `.env.local` (or runner-provided env) to include `ANTHROPIC_API_KEY` and any other live target vars.
