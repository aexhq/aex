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
`0.25cpu-1gb` for `size`). The CLI equivalents are `--runtime <kind>` and
`--runtime-size <size>`.

Runtime choice changes scheduling, cold-start behavior, capacity sourcing, and
price—not the LLM request, tools, files, controls, lifecycle, or stable error
contract. An explicit runtime request is never silently replaced with another
runtime. If it is unavailable for your workspace or the selected size, the
submission fails before execution.

Use `aex.whoami().runtimeCapabilities` (CLI: `aex whoami --json`) to inspect the
authenticated runtime kinds and sizes currently available to your workspace.
The capability hash identifies the exact availability document used by the
service. During a staged rollout a runtime may be part of the SDK vocabulary
without yet appearing in your workspace's available set.

## Selection

### TypeScript

```ts
import { Models, Providers, RuntimeKinds, Sizes } from "@aexhq/sdk";

const capabilities = (await aex.whoami()).runtimeCapabilities;
if (!capabilities?.availableRuntimeKinds.includes(RuntimeKinds.LAMBDA)) {
  throw new Error("Lambda runtime is not available for this workspace");
}

await aex.start({
  provider: Providers.OPENAI,
  model: Models.GPT_4_1,
  message: "Summarise the attached files.",
  runtime: {
    kind: RuntimeKinds.LAMBDA,
    size: Sizes.CPU_0_25_1GB
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
  --runtime-size 0.25cpu-1gb \
  --prompt "Summarise the attached files." \
  --follow
```

Events, files, streaming/replay, controls, cleanup, and downloads use the same
SDK and CLI surface for every runtime and provider. For the exact supported
model list, use the generated
[provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/).
