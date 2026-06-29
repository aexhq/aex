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
| Doubao China | `Providers.DOUBAO_CN` |

All submissions run on the managed runtime. There is no public runtime selector; omit `runtime`.

## Selection

### TypeScript

```ts
import { Models, Providers } from "@aexhq/sdk";

await aex.submit({
  provider: Providers.OPENAI,
  model: Models.GPT_4_1,
  prompt: "Summarise the attached files.",
  secrets: { apiKeys: { openai: process.env.OPENAI_API_KEY! } }
});
```

### CLI

```bash
aex run \
  --api-token "$AEX_API_TOKEN" \
  --provider openai \
  --openai-api-key "$OPENAI_API_KEY" \
  --model gpt-4.1 \
  --prompt "Summarise the attached files." \
  --follow
```

Events, outputs, cleanup, and downloads use the same SDK and CLI surface for
every provider. For the exact supported model list, use the generated
[provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/).
