---
title: Providers & Runtimes
description: How provider selection maps to aex runtime execution.
icon: Network
---

aex exposes one customer-facing submission shape across providers:

| Provider | Default runtime |
| --- | --- |
| `anthropic` | managed |
| `deepseek` | managed |
| `openai` | managed |
| `gemini` | managed |
| `mistral` | managed |

The managed runtime means aex starts a per-run agent process in an isolated managed runtime and routes upstream model calls through the BYOK provider-proxy.

The optional `runtime` field accepts only `"managed"`; omitting it also uses the managed runtime. `runtime: "native"` is rejected as an invalid runtime selector.

```ts
import { RunModels } from "@aexhq/sdk";

await aex.submit({
  provider: "openai",
  model: RunModels.GPT_4_1,
  prompt: "Summarise the attached files.",
  secrets: { apiKey: process.env.OPENAI_API_KEY! }
});
```

Events, outputs, cleanup, and downloads use the same SDK and CLI surface regardless of provider.

For exact provider statuses, routing cells, and evidence pointers, use the generated [provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/).
