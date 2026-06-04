---
title: Providers & Runtimes
description: How provider selection maps to aex runtime execution.
icon: Network
---

aex exposes one customer-facing submission shape across providers:

| Provider | Default runtime |
| --- | --- |
| `anthropic` | Goose Managed |
| `deepseek` | Goose Managed |
| `openai` | Goose Managed |
| `gemini` | Goose Managed |
| `mistral` | Goose Managed |

Goose Managed means aex starts a per-run Goose process in an isolated managed runtime and routes upstream model calls through the BYOK provider-proxy.

The optional `runtime` field accepts only `"managed"`; omitting it also uses Goose Managed. `runtime: "native"` is rejected as an invalid runtime selector.

```ts
await client.submitRun({
  provider: "openai",
  model: "gpt-4.1",
  prompt: "Summarise the attached files.",
  secrets: { openai: { apiKey: process.env.OPENAI_API_KEY! } }
});
```

Events, outputs, cleanup, and downloads use the same SDK and CLI surface regardless of provider.

For exact provider statuses, routing cells, and evidence pointers, use the generated [provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/).
