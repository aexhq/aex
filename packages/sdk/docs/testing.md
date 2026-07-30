---
title: Testing
---

# Testing

The repository keeps deterministic package tests separate from clean-install
and live user tests:

```text
bun run build
bun run lint
bun run typecheck
bun run test:unit
bun run test:validate
bun run test:user:offline
```

The offline user suite packs both `@aexhq/sdk` and `@aexhq/cli`, installs them
into a fresh Bun project, and exercises the strict v1 resource flow without a
hosted API. The manually dispatched live workflow runs the declared dev
scenario with `AEX_API_URL` and `AEX_API_KEY`.
