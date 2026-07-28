---
title: aex architecture and the open-core boundary
description: How an aex session executes — the brain/hands split, ports, the journal and its projection, and the runtime hosts — and exactly which concerns live in this repository versus the private hosted plane.
keywords:
  - architecture
  - open core
  - agent runtime
  - session
  - boundary
audience: contributors, integrators, and implementation agents
status: accepted
related:
  - references/glossary.md
  - references/internal-protocol.md
  - references/repo.md
  - references/rules.md
---

# Architecture

Read [`glossary.md`](glossary.md) first if `brain`, `hands`, or `plane` are new
to you. This page assumes those four paragraphs.

## The shape of the thing

An aex session is a durable, resumable thread. A model drives it, real tools
execute inside an isolated runtime, and every event, file, and unit of cost is
recorded. Three properties fall out of that and shape everything below:

1. **A session outlives any process that runs it.** State lives in a journal and
   a checkpoint, never only in memory. Any runtime that can read those can
   continue the session.
2. **The code being run is hostile by assumption.** Customer code and
   model-authored code share a runtime, so the runtime holds no durable
   credential and makes no network decision. See
   [`internal-protocol.md`](internal-protocol.md).
3. **A terminal run event is a consistency boundary.** When `RUN_FINISHED` is
   visible, the checkpoint, files, usage, cost, messages, and session status are
   all committed and mutually consistent. Nothing is eventually consistent
   across that line.

## Deciding and doing are separate

The engine splits an agent turn into two halves with a narrow seam between them.

```
                 ┌──────────────────────────────────────────┐
   model  ◄─────►│  brain — the loop                        │
                 │  assemble context → call model → plan    │
                 │  ONE tool call → fold the result         │
                 └───────────────┬──────────────────────────┘
                                 │ hands.execute(call, { timeoutMs })
                                 ▼
                 ┌──────────────────────────────────────────┐
                 │  hands — the tool executor               │
                 │  run it, return blocks + isError + ms    │
                 └───────────────┬──────────────────────────┘
                                 │  every outbound request
                                 ▼
                 ┌──────────────────────────────────────────┐
                 │  the managed boundary (not in this repo) │
                 │  resolve session → validate → inject     │
                 │  credential → forward → meter            │
                 └──────────────────────────────────────────┘
```

**Why the seam is there.** The brain is where model output becomes a decision;
the hands are where a decision becomes a side effect. Keeping them behind a
frame-shaped port means the two halves can run in different processes and
different trust domains without the loop knowing, and it means a tool result is
a value the loop folds rather than a mutation the loop performs.

One consequence worth knowing: the `subagent` builtin does not run in the tool
executor. Spawning a child session is a control-plane act, so it is routed to a
separate runner port. The engine owns the seam; it never makes the call itself.

## Ports, not integrations

The brain loop depends on ports — narrow interfaces it is handed — rather than on
any concrete service. Model access, tool execution, journalling, subagent
spawning, and telemetry are all ports.

That is what lets the same engine run on four different runtimes without a
per-runtime fork, and it is what makes the engine testable without a network: a
test supplies its own port implementations.

## Journal, projection, and checkpoint

The brain does not emit customer-facing events. It writes journal entries, and a
separate pure projection turns those entries into the ordered event stream a
client sees.

**Why the indirection.** The journal is the durable truth and can be replayed;
the event stream is a view. Reconnecting a client mid-run replays from the
journal through the same projection that produced the live stream, so a
reconnecting reader and a live reader cannot diverge. Ordering is a property of
the journal, not of delivery.

A **checkpoint** is the committed filesystem state at the end of a run. Session
file reads pin a checkpoint revision, so a read is answered from a specific
committed state rather than from whatever a live runtime currently holds.

## Runtimes

A session names the runtime it wants. The choice changes placement, isolation,
and metering — never agent capability or the customer-visible contract.

| Runtime | Character |
| --- | --- |
| `container` | The default managed container runtime. |
| `spot_container` | The same runtime on interruptible capacity. |
| `lambda` | A function runtime that idles to zero cost. |
| `microvm` | A micro-VM runtime for stronger isolation. |

Explicit runtime requests never silently fall back or migrate to another host.
Which capabilities each runtime supports is generated from the code rather than
asserted in prose: see `packages/sdk/docs/provider-runtime-capabilities.md`.

## Model access

aex holds one platform-owned model-gateway credential per plane and routes every
model call through it. Callers name a model by its `creator/model` gateway slug
and supply no provider API key; a submission carrying a provider key or a
provider selection is rejected.

This is a product decision with an architectural consequence: there is no
per-provider code path in the engine, no provider credential in the runtime, and
one host to allow rather than a fan-out.

## Composition

Reusable inputs — files, skills, tools, instructions — are published once into a
workspace as immutable, content-addressed, named versions, and then referenced
from a submission as exact pinned refs under `assets`. Publishing and using are
separate operations.

**Why.** A submission that carried its inputs inline would make every run's
behaviour irreproducible and every large input a repeated upload. A pinned ref
makes a run's exact inputs recoverable after the fact, which is what makes a
session auditable at all.

## The open-core boundary

This repository is Apache 2.0 and holds:

- **the public contract** — wire schemas, the event envelope, id formats, and the
  generated OpenAPI document (`packages/contracts`);
- **the clients** — the TypeScript SDK and the CLI (`packages/sdk`,
  `packages/cli`);
- **the engine** — the session loop, the tool kit and the builtin tools, the
  runtime adapters, the LLM protocol layer, and the runtime-side protocol
  described in [`internal-protocol.md`](internal-protocol.md). These packages are
  being extracted from the private history with their commit lineage preserved;
  the repository README lists what has landed.

The private repository holds the hosted plane:

- accounts, authentication, and API-key issuance;
- billing, pricing, margins, and reconciliation;
- admission, scheduling, and quota;
- the database schema and its migrations;
- the dashboard and its backend;
- the egress boundary implementation;
- every infrastructure definition.

### What that means in practice

**Running the hosted plane yourself is out of scope for this repository.** The
engine is published, its contract is published, and its protocol is published.
Assembling a control plane around them is not supported here, and no part of
this documentation should be read as offering it.

`scripts/cicd/check-public-boundary.mjs` enforces that in the docs it scans:
`README.md`, `SECURITY.md`, `packages/sdk/README.md`, and `packages/sdk/docs`.
It is a lint rule, not a promise in either direction.

**Substrate is not part of the public contract.** The public SDK and CLI expose
the runtime kind and size because those are intentional product fields. Storage,
regions, queues, object keys, credentials, and deployment identifiers are not
public contract, and a change to any of them is not a public API change.

**The seam is enforced, not just documented.** The generation direction runs one
way — Zod schemas produce the OpenAPI document, which produces types, which are
committed and freshness-gated — so a hosted change that alters the wire format
cannot land without the public artifact changing with it.

## Design rules the code actually holds itself to

- **One behavioural contract, several hosts.** A runtime is a placement decision.
  Two hosts that disagree about behaviour is a bug in one of them, not a
  documented difference.
- **One byte-upload path.** Every workspace resource kind uses the same
  presigned direct upload and finalize into immutable content-addressed assets.
- **One submission resource shape.** Reusable inputs appear only as exact refs
  under `assets`. Builtin tools, secrets, and MCP configuration stay separate
  fields.
- **One evidence path.** Every writer commits through the same control decision
  and reads through the same journal and projection helpers. A second way to
  record what happened is a second version of what happened.
- **Ports over integrations.** A dependency the loop cannot substitute in a test
  is a dependency the loop should not have.
