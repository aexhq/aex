---
title: aex public v1 architecture and repository boundary
description: The session-centric launch contract for messages, BYOK execution, retained generations, files, operations, and telemetry.
keywords:
  - architecture
  - public contract
  - session
  - operations
  - boundary
audience: contributors, integrators, and implementation agents
status: accepted
last_verified: 2026-08-11
related:
  - references/glossary.md
  - references/repo.md
  - references/rules.md
  - references/backlog.md
---

# Public v1 architecture

This page describes the supported public contract, not the hosted substrate.
The strict schemas under [`api/schemas/`](../api/schemas/) are the wire
authority. [`api/generated/`](../api/generated/),
[`crates/aex-wire/`](../crates/aex-wire/),
[`packages/wire/`](../packages/wire/), and the generated SDK resources and
models are derived from that authority and never edited by hand.

## Contract rules

1. **The session is the execution resource.** There is no public run or turn
   resource. A session accepts one user message at a time and may accept another
   after the current activity finishes or is cancelled.
2. **Mutations are explicit.** Session creation, message admission, manual
   lifecycle changes, live-file transfers, telemetry exports, and deletion are
   named calls with their declared replay identity.
3. **Ordinary reads are observational.** Live-file calls are action-shaped
   because they may resume the exact retained generation.
4. **One generation means one continuity boundary.** A session has at most one
   retained provider generation. Launch has no snapshot, clone/fork, crash
   recovery, trash, or restore path.
5. **Implementation is resolved, not selected.** Customers choose documented
   provider, model, capacity, network, package, and file inputs. Hosted service
   placement remains implementation detail.

## Sessions and messages

Session creation pins an exact BYOK provider credential, qualifies the
provider-native model, launches one generation, and returns only after the
session reaches its readiness boundary. It does not admit a prompt. A separate
message call accepts one text body of at most 24,576 UTF-8 bytes and starts the
session's current work.

Public session status is:

- `idle`
- `running`
- `suspending`
- `suspended`
- `resuming`
- `terminating`
- `terminated`
- `deleting`

Complete sealed messages are durable and listed in immutable seal-visibility
order. Internal agents, turns, and provider effects do not create public run
identities. Streaming and telemetry observe session activity; reading them does
not start work.

## Configuration and BYOK

Creation requires `provider`, exact provider-native `model`, and
`providerCredentialId`. Provider credentials are dedicated encrypted,
write-only BYOK resources. The platform does not supply a managed model key and
does not guess between credentials.

Supported compute sizes are `512mb`, `1gb`, `2gb`, `4gb`, and `8gb`.
`resolvedConfig` reports the qualified catalog revision, capacity, network,
packages, registered-file selection, and immutable lifecycle policy. Guest
network mode is `none` or `public_internet`; it does not select a different
runtime implementation.

Launch has no generic secret vault or typed skill, tool, instruction, or MCP
registry. Those materials may be opaque registered workspace files. There is
also no public interactive approval policy or approval resource.

## Retained-generation lifecycle

The provider generation has a hard lifetime of 28,800 seconds from
`launchedAt`. Suspension never extends that deadline. An idle generation
suspends automatically after exactly 180 seconds and automatically resumes for
the next message or live-file call. Manual suspend and resume use durable
operations and preserve the same generation.

Termination permanently destroys compute and live files while retaining
session metadata, sealed messages, accounting, and telemetry. Runtime loss has
the same execution-state outcome and records `runtime_lost`; the service does
not reconstruct an ambiguous partial activity. Irreversible deletion is a
separate durable operation that removes the declared session-scoped content and
leaves only the minimal tombstone plus independent aggregate facts.

The caller supplies durable operation identities with the canonical SDK helper:

```ts
import { newId } from "@aexhq/sdk";

await aex.sessions.sessionSuspend({
  sessionId,
  body: {},
  operationId: newId("operation"),
});
```

## Tools and subagents

The built-in model-tool surface is exactly `read_file`, `edit_file`,
`write_file`, and Bash. All four execute inside the session MicroVM. Hosted web
tools, custom hosted tools, generic secret injection, and human approval round
trips are not launch features.

Subagents may be created recursively within one customer session. They share
that session's generation and lifetime; they do not create independent public
sessions or runs.

## Durable workspace files and live files

Registered workspace files are durable opaque resources addressed by exact,
case-sensitive name. Session creation selects exact current revisions and
materializes them into the MicroVM. Later registry changes do not silently
rewrite an admitted session.

Live session files exist only in the retained generation. Exact list and stat
plus bounded resumable upload and download may resume that generation. The
multipart contract uses canonical `sha256:<64 lowercase hex>` content hashes,
exact-generation bindings, fixed part bounds, and final digest verification.
Termination, expiry, or runtime loss destroys these files. There is no
persisted-session file root and no operation that copies live files into one.

Short-lived downloads bind their object or range, canonical decimal lengths,
expiry, measurement identity, and immutable whole-object hash. Clients verify
that evidence before accepting complete content.

## Telemetry and usage

Events, logs, spans, metrics, traces, gaps, and combined telemetry remain typed
durable observation surfaces. They can be queried at session or workspace scope
and survive suspension or termination independently of ephemeral execution
state. Large extracts are explicit durable telemetry-export operations with
separate short-lived download grants.

Usage is a typed regional resource. Account and billing clients aggregate those
facts without changing the regional session contract.

## Repository boundary

This Apache-2.0 repository owns the strict public contract and generated
bindings, public SDK and CLI, runtime/service/tooling code, public documentation,
infrastructure modules, and black-box user tests. Hosted authentication,
scheduling, execution, storage, billing, and observability internals may change
without changing the public API when the strict wire behavior stays identical.

Generation flows one way from authored schemas. Run
`cargo run -p aex-contract-gen -- check` to prove generated OpenAPI, JSON Schema,
Rust, TypeScript wire, and SDK surfaces match the same contract.

## Design rules

- One public session; no public run resource.
- One active user message and at most one retained generation per session.
- One exact BYOK credential and provider-native model binding.
- One durable opaque registered-file abstraction; live files are ephemeral.
- One exact four-tool launch catalog inside the MicroVM.
- One typed observation model across query, stream, and export.
- One durable operation model for long-running mutations.
