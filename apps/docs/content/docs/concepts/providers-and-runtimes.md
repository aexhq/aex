---
title: Providers & runtimes
description: How provider selection maps to managed runtime execution.
icon: Network
---

aex exposes one submission shape across supported providers:

| Provider | Selector |
| --- | --- |
| Anthropic | `Providers.ANTHROPIC` |
| DeepSeek | `Providers.DEEPSEEK` |
| OpenAI | `Providers.OPENAI` |
| Gemini | `Providers.GEMINI` |
| Mistral | `Providers.MISTRAL` |
| OpenRouter | `Providers.OPENROUTER` |
| Doubao | `Providers.DOUBAO` |

All submissions run on the managed runtime. The optional `runtime` option picks
a managed machine-size preset — use `Sizes.*` in TypeScript (e.g.
`runtime: Sizes.SHARED_0_25X_1GB`) or `--runtime-size` in the CLI. Omit it for
the default size; there is no alternative runtime backend to select.

## Selection

### TypeScript

```ts
import { Models, Providers } from "@aexhq/sdk";

await aex.start({
  provider: Providers.OPENAI,
  model: Models.GPT_4_1,
  message: "Summarise the attached files.",
  apiKeys: { openai: process.env.OPENAI_API_KEY! }
});
```

### CLI

```bash
aex start \
  --api-key "$AEX_API_KEY" \
  --provider openai \
  --openai-api-key "$OPENAI_API_KEY" \
  --model gpt-4.1 \
  --prompt "Summarise the attached files." \
  --follow
```

Events, files, cleanup, and downloads use the same SDK and CLI surface for
every provider. For the exact supported model list, use the generated
[provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/).
