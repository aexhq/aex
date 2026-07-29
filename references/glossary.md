---
title: aex glossary
description: The vocabulary the aex source uses without defining — brain, hands, plane, session versus run, the internal virtual hosts, and the runtime names — with the file that owns each term.
keywords:
  - glossary
  - terminology
  - brain
  - hands
  - plane
  - runtime
audience: contributors and implementation agents
status: accepted
related:
  - references/architecture.md
  - references/internal-protocol.md
  - references/repo.md
---

# Glossary

The aex source uses a small private vocabulary heavily and defines it nowhere.
`brain` appears in 231 runtime source files; `hands` and `plane` are similarly
load-bearing. Renaming them is not on the table — the names reach exported
identifiers across the whole runtime, and a rename would break every consumer to
buy nothing a paragraph cannot buy. So this page is the paragraph.

Each entry names the file that owns the concept. Where the file has not yet
landed in this repository, the entry says so rather than linking a path that
does not exist.

## The execution vocabulary

### brain

The model-driving loop. The brain decides what to do next: it assembles context,
calls the model, reads the response, plans exactly one tool call per step, and
folds the result back into the session state. It holds no credentials of its own
and reaches nothing directly — everything it wants from the outside world goes
through a port.

"Brain" is the loop, not a process. It runs inside the session runtime
(`packages/container-runtime`, once extracted) but the loop itself is
host-neutral and lives in `packages/agent-session`.

### hands

The tool-execution side. Where the brain plans one call, the hands execute it and
return a result frame. The seam between them is a dispatcher port: the brain
calls `hands.execute(call, { timeoutMs })` and receives blocks, an error flag,
and a duration. Nothing else crosses.

The split exists so the deciding half and the doing half can run in different
trust domains and different processes. The `subagent` builtin is the documented
exception: it is not executed by the tool executor but routed to a child-session
runner, because spawning a child is a control-plane act rather than a tool call.

### step / turn / run / session

Four nested units, and they are not interchangeable:

| Unit | What it is |
| --- | --- |
| **step** | One brain iteration: one model call, at most one tool call, one fold. |
| **turn** | One user message and everything the agent does in response to it. |
| **run** | One submission's execution, ending in a terminal `RUN_FINISHED`. |
| **session** | The durable, resumable thread that runs belong to. |

A session's `status` describes whether the thread can progress, not whether the
last run succeeded. A run's terminal event is the consistency boundary:
checkpointed files, usage, cost, messages, and session state are all committed
before it is emitted.

### checkpoint

The committed filesystem state of a session at the end of a run. Session file
reads are checkpoint-aware: you read the state a specific run committed, not
whatever a live runtime happens to hold.

### journal

The append-only record of what a session did, written to object storage. The
brain journals; the projection turns journal entries into the customer-facing
event stream. Reachable from the runtime through `journal.internal` — see
[`internal-protocol.md`](internal-protocol.md).

## The deployment vocabulary

### plane

A complete, independent deployment of the hosted platform. There are exactly
two, `dev` and `prd`, and they are frozen into the wire format: an API key is
`aex_<plane>_<regionCode>_<workspaceId>_<secret>_<crc>`, so the SDK derives its
target plane from the key without a network call
(`packages/contracts/src/api-key.ts:23`).

A plane is **not** an environment variable and **not** localhost. `dev` is a
remote deployment; a developer machine running a local stack is a third thing
that is neither plane.

### region code

The frozen four-character region field inside an API key — `euw1`, `usw1`,
`apne1` (`packages/contracts/src/api-key.ts:32`). Frozen because a code is a
permanent field of every key ever minted with it.

### workspace

The tenancy boundary. Resources, secrets, spend, and sessions all belong to a
workspace, and an API key names exactly one.

## The runtime vocabulary

### runtime

Where a session's tools actually execute. The names appear in submissions and in
capability matrices:

| Name | What it is |
| --- | --- |
| `container` | A managed container runtime. The default. |
| `spot_container` | The same runtime on interruptible capacity. |
| `lambda` | A function runtime that idles to zero cost. |
| `microvm` | A Firecracker-class micro-VM runtime. |

Which runtimes support which capabilities is generated, not asserted in prose:
see `packages/sdk/docs/provider-runtime-capabilities.md`.

### runtime size

A named CPU and memory allocation for a session runtime, selected per session
rather than per account.

### subagent

A child session spawned by a parent session through the `subagent` builtin.
Children are separate submissions with their own lifecycle and their own
identity; the parent receives a typed result and, optionally, files. Lineage is
explicit — a child knows its parent.

### skill / instructions / tool / file

The four kinds of reusable, version-pinned workspace resource a submission can
reference under `assets`. Publishing is separate from using: you publish a
resource once and pin a specific version into any number of sessions.

## The boundary vocabulary

### `*.internal`

Six virtual hostnames the runtime addresses — `egress.internal`,
`web.internal`, `llm.internal`, `journal.internal`, `events.internal`, and
`aex.internal`. They do not resolve on the public internet and are not meant to.
Each one is a request the untrusted runtime makes and a managed boundary
terminates, validates, and credentials.

They are documented in full in [`internal-protocol.md`](internal-protocol.md).
Read that page before concluding that published code references a missing
service.

### egress boundary

The single outbound network path for untrusted session code. Its implementation
is part of the hosted plane and is not published; its protocol is, because the
runtime that speaks it is.

### managed model access

aex holds one platform-owned model-gateway credential per plane and routes every
model call through it. Customers name a model by its `creator/model` slug and
supply no provider key. A submission carrying a provider key or a provider
selection is rejected.

## Terms that mean something narrower than they look

| Term | Not what you would guess |
| --- | --- |
| **runner image** | The tool bundle, not a Dockerfile. The image build lives with the container runtime. |
| **canary** | An immutable npm prerelease bound to an exact source SHA. Not a traffic-splitting deploy. |
| **promotion** | Moving an existing canary version to the stable dist-tag. It publishes nothing new. |
| **plane identity** | The single commit a hosted release deploys, not a credential. |
