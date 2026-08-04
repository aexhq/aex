---
title: Rust-native rewrite — implementation handoffs
description: Index of the per-stream implementation handoffs recording what each area of the Rust-native rewrite landed, deliberately deferred, published for peers, and owes to other streams.
keywords:
  - rust
  - rewrite
  - handoff
  - implementation
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-04
related:
  - references/README.md
  - references/architecture.md
---

# Implementation handoffs

One document per implementation stream of the Rust-native rewrite. Each records
what the stream landed, what it deliberately deferred and why, the exact types
it publishes for peers, what it needs from other streams, and the decisions it
took. They sit beside the code they describe; the accepted design and the plans
compiled from it live in the parent workspace under
`references/rust-native-rewrite-2026-07-31/`.

Read the handoff for an area before changing its code — several record a
decision that deviates from the original plan, with the evidence for it.

| Handoff | Area |
| --- | --- |
| [`contracts.md`](contracts.md) | OpenAPI authoring, the generator, generated wire, server traits and client |
| [`central-identity.md`](central-identity.md) | Identity, control, authorization assertion, Data API transport, central HTTP |
| [`central-finance.md`](central-finance.md) | Balanced ledger, exact rating, Stripe edges, schema administration |
| [`regional-domains.md`](regional-domains.md) | Session, operation, workspace, content and secret pure domains |
| [`regional-stores.md`](regional-stores.md) | DynamoDB authority adapters, S3 content, KMS secret custody |
| [`regional-services.md`](regional-services.md) | Regional HTTP composition, finite APIs, stream service, lifecycle workers |
| [`brain.md`](brain.md) | Journal fold, lease and fence, split-phase effects, subagents, the mux |
| [`architecture-performance-v1.md`](architecture-performance-v1.md) | ARM64-only, performance-first runtime placement, Brain critical path, tool lanes, scheduling, and release qualification |
| [`implementation/brain-activation.md`](implementation/brain-activation.md) | Test-first activation/cache/admission implementation slice |
| [`implementation/tool-fabric.md`](implementation/tool-fabric.md) | Typed Brain-control, provider/web/MCP, Hands, and durable tool-fabric implementation slice |
| [`implementation/arm-release.md`](implementation/arm-release.md) | ARM64 artifact, readiness, load-receipt, and rollback implementation slice |
| [`providers.md`](providers.md) | Signed model catalog and the six direct BYOK provider adapters |
| [`tools-mcp.md`](tools-mcp.md) | Tool catalog, Brain-managed web, MCP adapter |
| [`hands.md`](hands.md) | Hands protocol, guest agent and tools, trusted control, true idle |
| [`observations.md`](observations.md) | Observation authority, OTLP admission, bounded query, export |
| [`usage.md`](usage.md) | Usage fact authorities, the four meters, the measurement probe API |
| [`clients.md`](clients.md) | TypeScript SDK, native CLI, dashboard, site, user tests |
| [`delivery.md`](delivery.md) | Release tool, CI lanes, artifact envelope, Terraform modules, evidence |
| [`test-architecture.md`](test-architecture.md) | Test-ownership metadata, derived registry, flake enforcement, fault model |
