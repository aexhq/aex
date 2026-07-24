---
title: Models & runtimes
description: How gateway model slugs map to managed runtime execution.
icon: Network
---

aex routes every model through the managed Vercel AI Gateway. You name a model
by its gateway `creator/model` **slug** and the platform's single managed key
handles the upstream provider relationship — there is no `provider` selector and
you never supply a provider API key.

```ts
model: "anthropic/claude-haiku-4-5"   // creator/model gateway slug
```

The slug is validated at the boundary by `parseModelSlug` (shape only). The
catalog is OPEN: a well-formed slug the gateway serves just works with zero code
changes; a slug this SDK does not recognize is still accepted and arbitrated by
the gateway at submit time.

All submissions run on a managed runtime. The optional `runtime` object has two
independent selectors:

- `runtime.kind` selects the execution backend: `RuntimeKinds.LAMBDA`
  (the default, `"lambda"`), `RuntimeKinds.SPOT_CONTAINER`
  (`"spot_container"`), or `RuntimeKinds.CONTAINER` (`"container"`).
- `runtime.size` selects a managed machine-size preset; use `Sizes.*` in
  TypeScript.

Omit either field to use its default (`lambda` for `kind` and
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
import { RuntimeKinds, Sizes } from "@aexhq/sdk";

const capabilities = (await aex.whoami()).runtimeCapabilities;
if (!capabilities?.availableRuntimeKinds.includes(RuntimeKinds.LAMBDA)) {
  throw new Error("Lambda runtime is not available for this workspace");
}

await aex.start({
  model: "openai/gpt-4.1",
  message: "Summarise the attached files.",
  runtime: {
    kind: RuntimeKinds.LAMBDA,
    size: Sizes.CPU_0_25_1GB
  }
});
```

### CLI

```bash
aex start \
  --api-key "$AEX_API_KEY" \
  --model openai/gpt-4.1 \
  --runtime lambda \
  --runtime-size 0.25cpu-1gb \
  --prompt "Summarise the attached files." \
  --follow
```

Events, files, streaming/replay, controls, cleanup, and downloads use the same
SDK and CLI surface for every runtime and model. For the model-access contract,
see the generated [model access reference](../provider-runtime-capabilities.md).
