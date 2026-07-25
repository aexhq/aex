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

- `runtime.kind` selects the execution backend: `RuntimeKinds.SPOT_CONTAINER`
  (the default, `"spot_container"`), `RuntimeKinds.CONTAINER`
  (`"container"`), or `RuntimeKinds.LAMBDA` (`"lambda"`).
- `runtime.size` selects a managed machine-size preset; use `Sizes.*` in
  TypeScript.

Omit either field to use its default (`spot_container` for `kind` and
`0.25cpu-1gb` for `size`). The CLI equivalents are `--runtime <kind>` and
`--runtime-size <size>`.

An explicit runtime request is never silently replaced with another runtime. If
the requested runtime cannot do what the submission asks — a capability it does
not support, a session deadline past its lifetime, a workspace larger than it
provisions — the submission fails before execution rather than running in a
degraded mode.

## What each runtime can actually do

Runtime choice is not free of behavioral consequences, and this SDK does not
claim otherwise. Every runtime publishes a **profile**: the capabilities it
performs, the limits it enforces, and the delivery semantics it guarantees. Read
it at runtime with `aex.whoami().runtimeCapabilities.profilesByRuntimeKind`
(CLI: `aex whoami --json`); the capability hash identifies the exact document
the service used.

Four differences are real, permanent, and cannot be equalized:

| Difference | Where it is published | What it means for you |
| --- | --- | --- |
| **Idle billing and cold start** | `profile.delivery.idleBilling`, `profile.delivery.coldStartClass` | This is the product reason the runtimes exist. `lambda` bills **zero** while a session is parked or waiting and cold-starts in seconds; the container runtimes bill **wall clock** for the whole session and cold-start in tens of seconds. |
| **`spot_container` runs side-effecting tools at least once** | `profile.delivery.toolExecution` | A Spot reclaim replays the interrupted step, so a tool with an external side effect may run more than once. `container` and `lambda` are `exactly-once`. If your tools are not idempotent, choose `container`. |
| **MicroVM disk and lifetime** | `profile.limits.maxWorkspaceBytes`, `profile.limits.maxSessionMs` | `lambda` runs in a MicroVM with a 32 GiB disk and an 8-hour hard lifetime. These are host limits, not policy, and admission rejects a submission that exceeds them. |
| **One atomic effect is capped** | `profile.limits.maxSingleEffectMs` | A single LLM call or tool call may run for at most 14 minutes on **every** runtime. On `lambda` the live budget can be shorter still, bounded by the remaining invocation time. An overrun fails that tool call with a typed error; it never silently drops the turn. |

Capabilities are declared per runtime and are either `supported` or
`unsupported` — there is no partial state. A capability the selected runtime
does not support is refused at admission, so a runtime can never advertise a
tool it cannot execute.

> **Availability today.** `lambda` is not generally available. It can finish an
> LLM turn but cannot yet execute a tool call, and its profile says so:
> `toolExecution`, `workspaceCheckpoint`, `workspaceFileCapture`,
> `streamingDeltas`, `approvalGate`, `postHook`, `mcpTools`, `scheduledWait`,
> `customerSecrets`, and `containedEgress` are all `unsupported`. The default is
> `spot_container` because it is the cheapest runtime that executes every tool.
> Check `availableRuntimeKinds` before naming a runtime: during a staged rollout
> a runtime may be part of the SDK vocabulary without appearing in your
> workspace's available set.

Subagents behave identically on every runtime: a subagent shares its parent's
workspace and sees the files the parent just wrote.

## Selection

### TypeScript

```ts
import { RuntimeKinds, Sizes } from "@aexhq/sdk";

const capabilities = (await aex.whoami()).runtimeCapabilities;
if (!capabilities?.availableRuntimeKinds.includes(RuntimeKinds.CONTAINER)) {
  throw new Error("Container runtime is not available for this workspace");
}

// Tools with external side effects should not run on interruption-tolerant
// capacity: check the published delivery semantics rather than assuming.
const profile = capabilities.profilesByRuntimeKind[RuntimeKinds.CONTAINER];
if (profile.delivery.toolExecution !== "exactly-once") {
  throw new Error("this workload needs exactly-once tool execution");
}

await aex.start({
  model: "openai/gpt-4.1",
  message: "Summarise the attached files.",
  runtime: {
    kind: RuntimeKinds.CONTAINER,
    size: Sizes.CPU_0_25_1GB
  }
});
```

### CLI

```bash
aex start \
  --api-key "$AEX_API_KEY" \
  --model openai/gpt-4.1 \
  --runtime container \
  --runtime-size 0.25cpu-1gb \
  --prompt "Summarise the attached files." \
  --follow
```

Events, files, streaming/replay, controls, cleanup, and downloads use the same
SDK and CLI **surface** for every runtime and model — the same calls, the same
shapes, the same stable error codes. What a given runtime will actually perform
behind that surface is the profile above. For the model-access contract, see the
generated [model access reference](/docs/reference/provider-runtime-capabilities/).
