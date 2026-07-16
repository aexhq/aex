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

All submissions run on a managed runtime. The optional `runtime` object has two
independent selectors:

- `runtime.kind` selects the execution backend: `RuntimeKinds.CONTAINER`
  (the default, `"container"`), `RuntimeKinds.SPOT_CONTAINER`
  (`"spot_container"`), or `RuntimeKinds.LAMBDA` (`"lambda"`).
- `runtime.size` selects a managed machine-size preset; use `Sizes.*` in
  TypeScript.

Omit either field to use its default (`container` for `kind` and
`shared-0.25x-1gb` for `size`). The CLI equivalents are `--runtime <kind>` and
`--runtime-size <size>`.

## Selection

### TypeScript

```ts
import { Models, Providers, RuntimeKinds, Sizes } from "@aexhq/sdk";

await aex.start({
  provider: Providers.OPENAI,
  model: Models.GPT_4_1,
  message: "Summarise the attached files.",
  runtime: {
    kind: RuntimeKinds.LAMBDA,
    size: Sizes.SHARED_0_25X_1GB
  },
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
  --runtime lambda \
  --runtime-size shared-0.25x-1gb \
  --prompt "Summarise the attached files." \
  --follow
```

Events, files, cleanup, and downloads use the same SDK and CLI surface for
every provider. For the exact supported model list, use the generated
[provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/).
