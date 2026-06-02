---
title: Providers & Runtimes
description: How provider selection maps to antpath runtime execution.
icon: Network
---

antpath exposes one customer-facing submission shape across providers:

| Provider | Default runtime |
| --- | --- |
| `anthropic` | Anthropic Native |
| `deepseek` | Goose Managed |
| `openai` | Goose Managed |
| `gemini` | Goose Managed |
| `mistral` | Goose Managed |

Native means the provider hosts the agent runtime. Goose Managed means antpath starts a per-run Goose process in an isolated managed runtime and routes upstream model calls through the BYOK provider-proxy.

The optional `runtime` field is an override, not a feature flag. `runtime: "managed"` opts Anthropic into Goose Managed. `runtime: "native"` is only valid where a provider has a native agent runtime. If a selected runtime cannot serve a submitted feature, antpath rejects the run instead of silently changing runtimes.

```ts
await client.submitRun({
  provider: "openai",
  model: "gpt-4.1",
  prompt: "Summarise the attached files.",
  secrets: { openai: { apiKey: process.env.OPENAI_API_KEY! } }
});
```

Events, outputs, cleanup, and downloads use the same SDK and CLI surface regardless of the runtime.

For exact provider statuses, routing cells, native feature parity, and evidence pointers, use the generated [provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/).
