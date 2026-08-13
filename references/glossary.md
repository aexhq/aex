---
title: aex public v1 glossary
description: Definitions for the session-centric launch contract, content, telemetry, region, and release terms.
keywords:
  - glossary
  - terminology
  - session
  - operation
  - telemetry
audience: contributors and implementation agents
status: accepted
last_verified: 2026-08-11
related:
  - references/architecture.md
  - references/repo.md
  - references/backlog.md
---

# Public v1 glossary

The strict schemas under `api/schemas/` own public fields and enumerations. This
page explains their terms without creating a second contract.

## Session and execution terms

### session

The customer-facing conversation and execution resource. A session accepts one
user message at a time, may accept later messages, and owns at most one retained
provider generation. There is no public run or turn resource.

### message

A complete sealed item in a session conversation. Launch admission accepts one
text user message of at most 24,576 UTF-8 bytes. Assistant and tool messages may
contain the contract's canonical built-in tool records.

### current work

The activity started by the session's currently admitted user message. It is
observable through session state, messages, streams, and telemetry, not through
a separate public run identity.

### operation

A durable record for a long-running mutation such as cancellation, manual
suspend/resume, termination, session deletion, telemetry export, or workspace
deletion. The caller mints its `op_` UUIDv7 replay identity with
`newId("operation")`.

### generation

The exact provider MicroVM retained by a session. It has a hard eight-hour
lifetime from launch, may suspend and resume without changing identity, and is
never reconstructed after loss.

### termination

Permanent destruction of the session's compute and generation-local live files.
Session metadata, sealed messages, accounting, and telemetry remain until a
separate deletion.

### deletion

An asynchronous irreversible mutation that removes the declared session-scoped
content and telemetry after terminating its generation. There is no trash,
restore, or recovery window.

### subagent

An agent orchestrated recursively inside one customer session. It shares the
session generation and lifetime and is not an independent public session or run.

## Workspace and content terms

### workspace

The tenancy and regional boundary for sessions, provider credentials, registered
files, operations, limits, telemetry, and usage.

### workspace key

An API key scoped to one workspace. Its value carries the workspace's compact
region code and indexed key identity alongside the secret; the SDK uses the
region code to route without a discovery request.

### region code

The workspace-key routing value: `use1`, `use2`, `usw2`, `apne1`, or `euw1`.

### registered workspace file

One durable opaque value addressed by an exact, case-sensitive name. A session
selects exact revisions at creation and receives copies in its MicroVM. Guidance,
skills, tool material, instructions, or MCP configuration may be ordinary files;
they are not separate typed registries.

### live file

A file in the session's exact retained generation. Live list, stat, upload, and
download may resume a suspended generation. The file is not persisted or
snapshotted and is lost with termination, expiry, or runtime loss.

### content hash

A canonical SHA-256 identity spelled `sha256:` followed by 64 lowercase
hexadecimal digits.

### download grant

A short-lived bearer URL with expiry, canonical decimal byte lengths,
measurement identity, range evidence when partial, and immutable content hash.
Possession authorizes access until expiry, so callers must not log or forward it.

## Configuration terms

### provider credential

A dedicated encrypted, write-only BYOK binding. Session creation pins one exact
credential; the platform neither supplies a managed model key nor guesses among
bindings.

### provider and model

The explicit direct BYOK provider and exact provider-native model id admitted
by the compiled models.dev catalog. Arbitrary provider base URLs are not
accepted.

### compute size

The public baseline selector: `512mb`, `1gb`, `2gb`, `4gb`, or `8gb`. Resolved
CPU, peak memory, disk, bandwidth, and connection limits are observable facts.

### Hands

The untrusted tool-execution side of a session MicroVM. Its raw network policy
is `none` or `public_internet`. The launch built-in model tools are exactly
`read_file`, `edit_file`, `write_file`, and Bash.

### resolved configuration

The immutable provider/model/credential binding, qualified catalog revision,
compute and network policy, selected registered files, packages, and lifecycle
policy returned for a session. It contains no public approval policy or generic
secret binding.

## Observation terms

### telemetry

The durable typed observation surface for events, logs, spans, metrics, traces,
and combined queries. It is independent of ephemeral session execution state.

### gap

An explicit record that admitted telemetry is incomplete. Callers can require
completeness rather than silently accepting missing observations.

### export

A durable operation that prepares a bounded telemetry extract. Export creation
and its short-lived download grant are separate actions.

## Deployment and release terms

### plane

An operational deployment target. `dev` is remote non-production and `prd` is
remote production; `localhost` is neither plane.

### canary

An immutable npm prerelease bound to an exact source commit. It is not a
traffic-splitting deployment.

### promotion

Moving already-published immutable package bytes to the stable npm dist-tag. It
does not rebuild or republish those bytes.
