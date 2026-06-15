---
title: Secrets & BYOK
description: Per-run custody for provider keys, MCP credentials, and proxy endpoint auth.
icon: KeyRound
---

aex is BYOK. Every run supplies exactly one provider key matching the selected `provider`, plus any MCP credentials or proxy endpoint auth values needed for that run. There is no default secret store in the SDK and no client-held provider-key state.

```ts
import { RunModels } from "@aexhq/sdk";

await aex.submit({
  provider: "mistral",
  model: RunModels.MISTRAL_LARGE_LATEST,
  prompt: "Compare the docs and return a short changelog.",
  secrets: {
    apiKey: process.env.MISTRAL_API_KEY!
  }
});
```

Secrets are excluded from the idempotency fingerprint. Rotating a provider key while retrying the same non-secret run request keeps the retry attached to the same logical run.

MCP credentials go in `secrets.mcpServers`. Custom HTTP credentials go in `secrets.proxyEndpointAuth`, paired with non-secret `proxyEndpoints` policy in the submission. The raw auth value never enters the runtime container; the managed proxy injects it on outbound calls.
