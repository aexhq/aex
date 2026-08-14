---
title: Rust-native rewrite — implementation handoffs
description: Index of the point-in-time implementation handoffs recording what each Rust-native rewrite stream landed before later clean cuts.
keywords:
  - rust
  - rewrite
  - handoff
  - implementation
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-14
related:
  - references/README.md
  - references/architecture.md
---

# Implementation handoffs

One point-in-time document per implementation stream of the Rust-native rewrite.
Each records what that stream landed, deliberately deferred, published for peers,
and decided at the time. Later clean cuts supersede several of these handoffs;
their frontmatter and opening banner say so while retaining the old body for
provenance. The current session-centered launch contract is
[`../architecture.md`](../architecture.md), with exactly 32 served routes and 14
release units in the authored registries.

Use these handoffs as historical decision evidence, not as a current inventory.
For a changed area, read its current accepted architecture and executable
registry first; a handoff's explicit superseded banner wins over statements in
its retained body.

| Handoff | Area |
| --- | --- |
| [`contracts.md`](contracts.md) | OpenAPI authoring, the generator, generated wire, server traits and client |
| [`central-identity.md`](central-identity.md) | Identity, control, authorization assertion, Data API transport, central HTTP |
| [`central-finance.md`](central-finance.md) | Historical: former ledger, rating, Stripe-edge, and schema-administration slice (superseded) |
| [`regional-domains.md`](regional-domains.md) | Session, operation, workspace, content and secret pure domains |
| [`regional-stores.md`](regional-stores.md) | Historical: former DynamoDB, S3, and KMS store slice (superseded) |
| [`regional-services.md`](regional-services.md) | Historical: former regional HTTP and worker slice (superseded) |
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
| [`clients.md`](clients.md) | Historical: former SDK, CLI, dashboard, site, and user-test slice (superseded) |
| [`delivery.md`](delivery.md) | Release tool, CI lanes, artifact envelope, Terraform modules, evidence |
| [`test-architecture.md`](test-architecture.md) | Test-ownership metadata, derived registry, flake enforcement, fault model |
