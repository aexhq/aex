---
# GENERATED FILE - do not edit.
# Written by `scripts/docs/generate-all.mjs` from `packages/sdk/docs/authentication.md`.
# Edit the source and run `bun run docs:generate`. A hand edit here is
# reverted by the next `bun run lint`, which regenerates via `prelint`.
title: Authentication
---

# Authentication

Create `Aex` with an API key. Workspace keys encode their region, so the SDK
selects the regional API automatically.

```ts
import { Aex } from "@aexhq/sdk";

const aex = new Aex(process.env.AEX_WORKSPACE_API_KEY!);
```

Use account credentials only for bootstrap resources such as organizations,
workspaces, API keys, and billing. Treat every key as a secret: keep it out of
source control, logs, metadata, and exception messages.

For a local or controlled endpoint, pass `baseUrl` explicitly:

```ts
const aex = new Aex({
  apiKey: process.env.AEX_WORKSPACE_API_KEY!,
  baseUrl: "https://eu-west-1.api.example.test"
});
```
