---
title: aex session-centered launch architecture and repository boundary
description: The launch contract for messages, BYOK execution, one optional sandbox, workspace files, MCP, billing, and retained telemetry.
keywords:
  - architecture
  - public contract
  - session
  - operations
  - boundary
audience: contributors, integrators, and implementation agents
status: accepted
last_verified: 2026-08-14
related:
  - references/glossary.md
  - references/repo.md
  - references/rules.md
  - references/backlog.md
---

# Session-centered launch architecture

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
2. **Mutations are explicit and current-value.** Session creation, message
   admission, workspace-file replacement, telemetry download, termination, and
   deletion each declare their replay identity. No public resource offers
   revision history, restore, or version selection.
3. **Execution identities stay internal.** Agents, effects, tool handles,
   Hands, generations, and immutable content hashes are durable
   correctness identities, not public resources.
4. **One optional sandbox per session.** It is enabled by default, prepared in
   the background, and suspended when ready and unused. Explicit opt-out creates
   no Hand and sandbox tools return a structured error.
5. **Implementation is resolved, not selected.** Customers choose a qualified
   official provider/model, sandbox policy, MCP servers, packages, and frozen
   file mounts. Hosted placement remains implementation detail.

## Sessions and messages

Session creation encrypts a write-only session-scoped BYOK key, qualifies the
provider-native model, freezes file mounts and MCP configuration, commits the
root agent, and durably requests default-on sandbox preparation. It returns
without waiting for VM provisioning and does not admit a prompt. A separate
message call accepts one text body and starts the session's current root work.
Caller-provided file content is registered in the workspace before its current
content identity can enter the frozen mount manifest; preparation downloads
that manifest from registry-backed content into the MicroVM.

Public session status is:

- `idle`
- `running`
- `terminating`
- `terminated`
- `deleting`

Sandbox readiness is a separate durable status: `disabled`, `requested`,
`ready`, `suspending`, `suspended`, `resuming`, or `lost`. Provisioning and
workspace-transfer details are telemetry progress rather than extra public-head
states. Sandbox loss produces ordinary tool errors; it does not terminate
durable agent state.

Complete sealed messages are durable and listed in immutable seal-visibility
order. Assistant streaming is an explicitly lossy preview protocol with
message identity, gap frames, and a strong-read reconciliation frame after the
assistant message commits. Internal agents, turns, and provider effects do not
create public run identities.

## Configuration and BYOK

Creation requires `provider`, exact provider-native `model`, and a write-only
`providerApiKey`. Provider and MCP secrets are encrypted and frozen for that
session. The platform does not supply a managed model key or expose reusable
provider-credential CRUD.

The candidate official provider families are OpenAI, Anthropic, DeepSeek, xAI,
Meta, Moonshot AI, and Alibaba. A family ships only when `rig-core` can reach its
official endpoint through a maintained native, OpenAI-compatible, or
Anthropic-compatible path and the same streaming/tool/structured-output
conformance suite passes. The current models.dev-derived catalog is overwritten
at build time; there is no public catalog revision or historical model row.

Supported compute sizes are `512mb`, `1gb`, `2gb`, `4gb`, and `8gb`.
`resolvedConfig` reports provider/model, sandbox capacity and network, packages,
registered-file mounts, MCP server names, and immutable lifetime/subagent
bounds. Guest network mode is `none` or `public_internet`; it does not select a
different runtime implementation.

Launch has no generic secret vault, typed skill/instruction registry, custom
executable product, direct model attachment API, or interactive approval
resource.

## Retained-generation lifecycle

The session has a hard maximum lifetime of 28,800 seconds. Sandbox suspension
never extends it. Preparation eagerly provisions the one logical Hand,
materializes the frozen workspace, and suspends the exact generation when no
tool call is waiting. Provisioning and
materialization never hold session creation open. The first waiting sandbox
tool call is the only caller that waits for preparation or resume readiness
before execution.

Termination permanently destroys sandbox compute while retaining session
metadata, sealed messages, accounting, and telemetry. Irreversible deletion is
a separate durable operation that removes session telemetry and
declared session-scoped content before leaving only a minimal tombstone plus
independent aggregate facts. Workspace-file registry values remain independent.

The caller supplies durable operation identities with the canonical SDK helper:

```ts
import { newId } from "@aexhq/sdk";

await aex.sessions.sessionTerminate({
  sessionId,
  body: {},
  operationId: newId("operation"),
});
```

## Tools and subagents

Tool Mux is the only execution boundary. The built-in model-tool surface is
`read_file`, `edit_file`, `write_file`, Bash, MCP, and `storage.persist` (wire
name `storage_persist`). Sandbox tools execute through the exact Hand
generation; `storage.persist` streams a selected sandbox file through trusted
code and overwrites the named workspace file without guest AWS credentials.
Every start is detached: Tool Mux returns a durable handle promptly, execution
continues asynchronously, and Brain polls for completion rather than holding an
HTTP connection to Tool Mux for the duration of a tool.

Remote Streamable HTTP MCP and sandbox-process MCP configuration is frozen per
session. Both transports are invoked on demand through the built-in MCP tool
and obey the same sandbox result-file contract as CLI-backed tools; Brain has
no direct inline MCP execution path. Unsupported or disabled sandbox work is
returned to the model as a structured tool result. Hosted web/search, custom
executable tools, generic secret injection, and human approval round trips are
not launch features.

Subagents are native durable agents in the same session, not public sessions or
runs. The root is depth 0; children may reach depth 3, and at most 12 non-root
identities may ever be allocated over the session lifetime. Concurrent tool and
child results commit into model context in original call order.

## Workspace files and large results

Registered workspace files are durable opaque resources addressed by exact,
case-sensitive name. Inline bytes may become ready immediately; HTTPS imports
and direct uploads publish a pending current intent and later become ready or
failed only if that intent is still latest. Every successful write overwrites
the public current value; prior values cannot be listed, selected, or restored.

Session creation first requires caller-provided content to be registered as
current workspace files, then resolves the selected names to private immutable
content hashes and normalized `/workspace` mount paths. That manifest is frozen
for the session and downloaded during asynchronous sandbox preparation; later
overwrites do not mutate a running sandbox. Arbitrary images, PDFs, videos,
archives, and other bytes are supported as files, but a message references
their paths rather than attaching provider-native content.

Tool results are bounded in model context and live telemetry. Large/full output
is written to a stable call-scoped sandbox path; the result contains the path,
hash, byte count, and truncation status. It is never uploaded to S3
automatically. Only an explicit `storage.persist` invocation registers and
persists selected output as a workspace file. Workspace-file and telemetry
downloads are short-lived, integrity-bound grants.

## Telemetry and usage

Assistant, tool, runtime, OpenTelemetry Log and Trace, usage, gap, and heartbeat
frames share one monotonic per-session stream. Producers use bounded nonblocking
ingress, so a slow client or telemetry/S3 outage cannot delay provider
consumption, tool completion, or decision commit. Loss is explicit through gap
frames. Compressed immutable S3 segments support bounded replay and short-lived
downloads and survive sandbox suspension or termination.

Trusted runtime/storage/transfer facts flow through one FIFO to the central
billing worker. Provider token usage is retained as zero-dollar BYOK model
observability with provider/model/session attribution; it has no rate path and
creates no money-ledger charge. The central surface exposes cards, prepaid
balance, manual top-up, transactions, and bounded usage coverage.

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
- One latest-only durable workspace-file abstraction; private immutable hashes
  exist only for correctness and frozen mounts.
- One default-on optional Hand and one Tool Mux boundary.
- Remote and sandbox MCP, with secrets encrypted per session.
- Native subagents bounded to 12 lifetime identities and depth 3.
- One typed retained observation path across preview, telemetry, replay, and
  download.
- Essential prepaid billing without subscriptions, plans, invoices, portal, or
  auto-top-up.
