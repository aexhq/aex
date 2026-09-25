# ADR-008: Keep typed-response orchestration in Aex

- Decision date: 2026-09-25
- Status: Accepted

Amends the SDK inheritance choice in the [production preview](0003-production-preview.md).
Brain's neutral session and Environment ownership under [ADR-001](001-ownership.md)
and [ADR-002](002-brain-composition.md) remains unchanged.

## Context

A typed application answer needs schema instructions, selection of a completed answer,
validation and a policy for correcting invalid results. These choices belong to the
platform composing the agent. They are heavier than sending a message or reading its
committed events, even when implemented in a client rather than a server.

Brain SDK 0.21 added this policy because Aex inherited its concrete session handles.
Keeping the server contract unchanged did not make the policy a low-level Brain operation.

## Decision

Aex owns the per-send `output: { type, maxRetries }` API, its inferred result and errors,
schema prompting, Zod validation and bounded corrective turns. Brain owns ordinary
session operations, event history, cancellation and provider-native model-format controls.
Tool input/output schemas remain part of the existing extension contracts.

The Aex client composes one Brain client. Its session handles wrap the exact handles
returned by Brain, retaining their state, Environment services and lifecycle cleanup.
Neutral methods and extension helpers delegate to Brain; Aex does not copy transport,
host registration, the session engine or its journal. Product request headers apply to
every initial and corrective send through the same client.

The per-send API preserves local Zod semantics and uses ordinary corrective turns.
It needs its caller alive, cannot prohibit Tool calls through a prompt, and is not an
atomic or resumable server operation. Hosted correction inside an ordinary Agentloop
remains a separate extension policy. Neither requires a Brain output mode.

## Alternatives and consequences

- Leaving an optional helper in Brain would keep application policy in the wrong owner.
- A standalone function would avoid wrappers but change Aex's established method syntax.
- Subclass factories, private-field access and prototype patches would couple Brain to
  downstream customization. Explicit composition preserves the ordinary public seam.

Existing Aex method calls retain their behavior, but Aex clients and handles are no longer
instances of Brain's concrete classes. Consumers use `AexSessionHandle` for product handles;
the re-exported `Brain` and `SessionHandle` remain unchanged upstream identities.
Standalone Brain callers may wrap a handle in `AexSessionHandle` to use this policy.
There is no wire, stored-session or compiled-extension migration.

## Sources

- [Brain PR 195](https://github.com/aexhq/brain/pull/195) introduced the upstream SDK feature.
- [Aex PR 168](https://github.com/aexhq/aex/pull/168) inherited it.
- Owner direction on 2026-09-25: typed responses belong to platforms such as Aex;
  Brain remains the low-level operations server.
- [SDK usage and migration](../../packages/sdk/README.md#structured-output).
