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
last_verified: 2026-08-14
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
user message at a time, may accept later messages, and may own one retained
sandbox generation. There is no public run or turn resource.

### message

A complete sealed item in a session conversation. Launch admission accepts one
text user message of at most 24,576 UTF-8 bytes. Assistant and tool messages may
contain the contract's canonical built-in tool records.

### current work

The activity started by the session's currently admitted user message. It is
observable through session state, messages, streams, and telemetry, not through
a separate public run identity.

### operation

A caller-minted `op_` UUIDv7 correlation and replay identity for asynchronous
session cancellation, termination, or deletion. There is no generic operations
resource or operation list/get API.

### generation

The exact sandbox MicroVM incarnation retained by a session when sandboxing is
enabled. It shares the session's hard eight-hour lifetime, may suspend and
resume without changing identity, and is never reconstructed after loss.

### termination

Permanent destruction of the session's sandbox compute and sandbox-local files.
Session metadata, sealed messages, accounting, and telemetry remain until a
separate deletion.

### deletion

An asynchronous irreversible mutation that removes the declared session-scoped
content and telemetry after terminating its generation. There is no trash,
restore, or recovery window.

### subagent

An agent orchestrated recursively inside one customer session. It shares the
session lifetime and is not an independent public session or run. The root is
depth 0, children may reach depth 3, and at most 12 non-root identities may be
allocated over the session lifetime.

## Workspace and content terms

### workspace

The fixed tenancy and regional boundary for sessions, registered files,
telemetry, and usage.

### workspace key

An API key scoped to one workspace. Its value carries the workspace's compact
region code and indexed key identity alongside the secret; the SDK uses the
region code to route without a discovery request. Workspace keys authorize only
the 19 regional session and file routes.

### dashboard session

The browser credential returned by the central OAuth exchange. It authorizes
authenticated central bootstrap, API-key, and essential billing calls and is
distinct from a workspace API key; the SDK never substitutes one credential for
the other. The OAuth exchange that creates it is the central unauthenticated
exception.

### region code

The workspace-key routing value: `use1`, `use2`, `usw2`, `apne1`, or `euw1`.

### registered workspace file

One latest-only durable opaque value addressed by an exact, case-sensitive name.
Inline bytes, an HTTPS URL, or a direct upload may replace its current value; no
prior value can be listed, selected, or restored. A session resolves selected
names at creation and freezes their current content into its sandbox mount.
Guidance, skills, tool material, instructions, images, PDFs, video, or other
arbitrary bytes may be ordinary files; they are not separate typed registries.

### sandbox file

A file inside the session's optional sandbox. There is no public live-file CRUD
API. The model reaches sandbox files through built-in tools or Bash; a large tool
result is also written to a stable call-scoped path and is not uploaded
automatically. `storage.persist` can explicitly overwrite a named registered
workspace file through trusted Tool Mux code.

### content hash

A canonical SHA-256 identity spelled `sha256:` followed by 64 lowercase
hexadecimal digits.

### download grant

A short-lived bearer URL with expiry, canonical decimal byte lengths,
measurement identity, range evidence when partial, and immutable content hash.
Possession authorizes access until expiry, so callers must not log or forward it.

## Configuration terms

### provider API key

A write-only BYOK value supplied during session creation, encrypted for that
session, and never returned. There is no reusable provider-credential resource;
the platform neither supplies a managed model key nor guesses among bindings.

### provider and model

The explicit direct BYOK provider and exact provider-native model id admitted
by the current compiled models.dev catalog through Rig. Candidate official
families are OpenAI, Anthropic, DeepSeek, xAI, Meta, Moonshot AI, and Alibaba;
arbitrary provider base URLs are not accepted.

### compute size

The public baseline selector: `512mb`, `1gb`, `2gb`, `4gb`, or `8gb`. Resolved
CPU, peak memory, disk, bandwidth, and connection limits are observable facts.

### Hands

The one optional untrusted tool-execution sandbox for a session. It is enabled
by default, prepared eagerly, and suspended when ready and unused. Its raw
network policy is `none` or `public_internet`. The launch built-in model tools
are `read_file`, `edit_file`, `write_file`, Bash, MCP, and `storage.persist`
(wire name `storage_persist`). Frozen remote and sandbox-process MCP servers
are both invoked through that built-in MCP tool.

### resolved configuration

The immutable provider/model binding, sandbox compute and network policy,
selected registered-file names, packages, MCP server names, and lifecycle and
subagent bounds returned for a session. It omits plaintext credentials, private
content hashes, and any public catalog revision.

## Observation terms

### telemetry

The typed per-session observation path shared by assistant previews, tool and
runtime events, OpenTelemetry logs and traces, usage, gaps, and heartbeats. It is
streamed live and retained as compressed immutable S3 segments for bounded
replay and download.

### gap

An explicit record that admitted telemetry is incomplete. Callers can require
completeness rather than silently accepting missing observations.

### telemetry download

A short-lived integrity-bound grant for retained telemetry bytes. Live stream,
bounded replay, and download are separate session routes; there is no generic
export or operations resource.

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
