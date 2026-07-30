---
title: aex public v1 glossary
description: Definitions for the strict-v1 session, execution, content, telemetry, region, and release terms used across the public repository.
keywords:
  - glossary
  - terminology
  - session
  - operation
  - telemetry
audience: contributors and implementation agents
status: accepted
related:
  - references/architecture.md
  - references/repo.md
---

# Public v1 glossary

The strict schemas under `packages/contracts/src/` own public field names and
enumerations. This page explains how the repository uses those terms without
creating a second contract.

## Session and execution terms

### session

The durable conversational and workspace boundary. Creating a session resolves
its configuration but does not run a prompt. Public session status is `idle`,
`running`, `awaiting_approval`, or `deleting`.

### message

An admitted item in a session conversation. Sending a user message returns both
the accepted message and the durable run created to process it.

### run

One durable execution admitted for one user message. A run progresses through
`queued` or `running` to `succeeded`, `failed`, `timed_out`, `cancelled`, or
`interrupted`. The run resource is the authority for its status and result.

### operation

A durable handle for a long-running mutation such as stop, persist, fork,
workspace discard, credential rebind, telemetry export, or deletion. `wait()`
returns the terminal operation; `result()` returns its typed success value or
throws its typed failure.

### fork

An explicit operation that creates an independent session from a source
session. The resulting session records lineage, but has its own lifecycle and
identity.

### subagent

An agent orchestrated inside a session by the builtin `subagent` and
`subagent_result` tools. It is not a separate customer session resource.

## Workspace and content terms

### workspace

The tenancy and regional content boundary. Registered resources, secrets,
sessions, operations, limits, and regional usage belong to one workspace.

### workspace key

An API key scoped to one workspace. Its value encodes only a region code and an
indexed key identity alongside the secret; it does not encode a plane or
workspace ID. `packages/contracts/src/api-key.ts` owns the parser.

### region code

The compact execution-region field carried by a workspace key: `use1`, `use2`,
`usw2`, `apne1`, or `euw1`. The SDK uses it to select the regional API without a
discovery request.

### registered resource

One current workspace value addressed by an exact, case-sensitive name. Files,
skills, tools, instructions, and MCP servers use overwrite-by-name `set`, `get`,
`list`, and `delete` semantics. Old values are not publicly addressable.

### persisted files

The latest durable file root produced by an explicit `persist()` operation.
Persisted reads never wake compute and expose no historical-revision API.

### live files

Files in the exact retained workspace generation. Live reads are action
surfaces: they may wake that retained generation and never substitute persisted
bytes when the generation is unavailable.

### download grant

A short-lived bearer URL plus any required headers and immutable content
evidence. Possession grants access until expiry; callers must not log or forward
it.

## Configuration terms

### compute size

The only public compute selector. The accepted values are `512mb`, `1gb`,
`2gb`, `4gb`, and `8gb`. CPU, peak memory, disk, bandwidth, and connection
capacity are service-derived facts returned in `resolvedConfig.compute`.

### Hands

The untrusted tool-execution side named by `network.hands`. Its public raw
network policy is either `none` or `public_internet`. Hands is not an execution
implementation that a caller selects.

### managed model

A model named by its `creator/model` slug. The hosted service brokers model
access; customer provider keys and provider-selection fields are not part of
the session request.

### resolved configuration

The complete effective configuration returned on a session. It records resolved
registered inputs, builtin tools and harness facts, approval and network policy,
packages, compute capacity, and continuity policy. Resolved facts are
observable; implementation details are not independently selectable.

## Observation terms

### telemetry

The combined observation surface for events, logs, spans, metrics, and traces.
The signal namespaces share typed filters, cursors, queries, streams, and
explicit completeness behavior.

### gap

An explicit record that admitted telemetry is incomplete. Query, stream, and
export callers can require completeness rather than silently accepting missing
observations.

### export

A durable operation that prepares a bounded telemetry extract. Export creation
and export download are separate actions.

## Deployment and release terms

### plane

An operational deployment target. `dev` is remote non-production and `prd` is
remote production; `localhost` is neither plane. Plane is not a field encoded
in a workspace key.

### canary

An immutable npm prerelease bound to an exact source commit. It is not a
traffic-splitting deployment.

### promotion

Moving an already-published immutable canary to the stable npm dist-tag. It
publishes no new package bytes.
