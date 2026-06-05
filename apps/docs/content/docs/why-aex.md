---
title: Why aex / how it compares
description: Where aex fits between low-level sandbox primitives and provider-native managed agents.
icon: Compass
---

aex is the serverless control plane for autonomous agent sessions. You declare
a run — model, prompt, skills, MCP servers, files, and optional output-capture
roots — submit it with your own provider key, and get back an ordered event
stream plus captured outputs. There is no infrastructure to operate and no
per-provider integration to maintain.

## The wedge

- **One multi-provider surface.** The same `submitRun` shape, the same typed
  event stream, and the same SDK/CLI verbs work across Anthropic, DeepSeek,
  OpenAI, Gemini, and Mistral. Switching providers is a field change, not a
  rewrite.
- **BYOK custody.** You bring your own provider key. It travels inline with
  each submission, is held only in run-scoped custody for the lifetime of the
  run, is excluded from idempotency hashing, and is targeted for
  cleanup/revocation at terminal. aex never persists tenant provider keys as
  workspace-level connections.
- **An ordered, durable event stream.** Every run emits one normalized event
  shape, recorded after secret redaction. You can tail it live or read it back
  later from the durable run record — the timeline is the same either way.
- **Cleanup by default.** Tracked runtime resources are reclaimed when a run
  reaches a terminal status, and `cleanupStatus` surfaces anything that could
  not complete. There is no opt-out and no resource you have to remember to
  tear down.

## How it compares

### Sandbox primitives (E2B, Modal, Daytona, and similar)

Sandbox products give you a programmable execution environment — a container or
VM you drive yourself. They are excellent when you want to own the agent loop,
the tool wiring, and the lifecycle.

aex sits one level up. You do not write the loop, mount the tools, or manage the
sandbox lifecycle; you submit a declarative run and observe it. The trade is
deliberate: less low-level control in exchange for a uniform multi-provider
surface, BYOK custody, a normalized event stream, and automatic cleanup out of
the box. If your goal is "run this agent task across providers and get a durable
record back," aex removes the integration layer you would otherwise build on top
of a sandbox.

### Provider-native managed agents

Provider-native managed agent runtimes are a strong fit when you have committed
to a single provider and want that provider's first-party features.

aex keeps the surface provider-neutral. The same submission shape and event
stream span every supported provider, so your application code does not fork per
provider, and you are not re-implementing skill packaging, credential handling,
output capture, and event normalization once per vendor. Provider-specific
support facts — which providers are live-verified versus accepted-but-unproven —
are tracked explicitly in the
[provider/runtime capability matrix](/docs/reference/provider-runtime-capabilities/).

## What aex is not

aex is intentionally scoped. It is **not** a general-purpose sandbox, a custom
agent loop, an interactive human-approval system, or a provider compliance
layer. Self-host and customer-cloud deployment are not supported product modes.
Provider retention, training-exclusion, and residency properties belong to the
provider account you bring. The full list of owned versus inherited behavior is
in [product capabilities and boundaries](/docs/guides/product-boundaries/).

## Next

- [Quickstart](/docs/guides/quickstart/) — install, auth, first run.
- [Providers & runtimes](/docs/concepts/providers-and-runtimes/) — how provider
  selection maps to runtime execution.
- [Product capabilities and boundaries](/docs/guides/product-boundaries/) — what
  aex owns, inherits, and does not support.
