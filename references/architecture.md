---
title: aex public v1 architecture and repository boundary
description: How strict v1 exposes explicit sessions, durable operations, persisted and live files, registered resources, telemetry, and one execution contract.
keywords:
  - architecture
  - public contract
  - session
  - operations
  - boundary
audience: contributors, integrators, and implementation agents
status: accepted
related:
  - references/glossary.md
  - references/repo.md
  - references/rules.md
---

# Public v1 architecture

This page describes the supported public contract, not the hosted substrate.
The strict schemas under [`api/schemas/`](../api/schemas/) are the wire
authority; [`api/generated/`](../api/generated/) and
[`crates/aex-wire/`](../crates/aex-wire/) and
[`packages/wire/`](../packages/wire/) are generated from them and never edited
by hand. The documentation under
[`apps/site/content/docs/`](../apps/site/content/docs/) is the user-facing
authority.

## Contract rules

Strict v1 follows four rules:

1. **User-visible mutation is explicit.** Creating a session, admitting a
   message, persisting files, forking, discarding a live workspace, rebinding
   credentials, exporting telemetry, and deleting resources are named
   operations.
2. **Reads remain observational.** Resource GET/list/query calls do not start
   compute or mutate customer state. Live-file access is action-shaped because
   it may wake the exact retained workspace generation.
3. **Long-running mutations are durable.** Their operation handles can be
   reopened, waited on, and resolved independently of the client connection
   that admitted them.
4. **Implementation is resolved, not selected.** A caller chooses supported
   capacity and policy inputs. The hosted execution implementation is not a
   public resource, selector, or fallback ladder.

## Sessions, messages, and runs

Session creation establishes a durable conversational and workspace boundary.
It does not run a prompt. A message admission creates the accepted message and
a durable run:

```ts
const session = await aex.sessions.create({ model: "openai/gpt-5" });
const { message, run } = await session.messages.send("Inspect the tests.");
const terminal = await run.result();
```

Session status is semantic:

- `idle`
- `running`
- `awaiting_approval`
- `deleting`

Run status is independently observable:

- `queued`
- `running`
- `succeeded`
- `failed`
- `timed_out`
- `cancelled`
- `interrupted`

The canonical run resource, not a projected stream event, owns terminal status,
result references, and typed failure. Streaming and telemetry are observation
surfaces over that execution; consuming a stream never starts a run.

Session lifecycle changes are explicit durable operations:

- `stop()` makes the session quiescent;
- `persist()` synchronizes selected live files into the durable file root;
- `fork()` creates an independent session with explicit lineage;
- `workspace.discard()` deliberately loses unpersisted live-workspace state;
- `delete()` closes admission and purges the session under its declared cascade
  behavior.

There is no public suspend/resume lifecycle or automatic end-of-run file
capture. The builtin subagent tools orchestrate agents inside one session; they
do not create a separate customer session resource. An independent session is
created only by an explicit session create or fork operation.

## Configuration and execution

A session request names a model by its managed `creator/model` slug. Customers
do not select a provider implementation or supply a provider API key.

Compute is requested only by capacity:

```ts
const session = await aex.sessions.create({
  model: "openai/gpt-5",
  compute: { size: "1gb" }
});
```

Supported sizes are `512mb`, `1gb`, `2gb`, `4gb`, and `8gb`.
`resolvedConfig.compute` returns the service-derived baseline and peak memory
and CPU, maximum disk, endpoint bandwidth, and concurrent-connection capacity.
Those resolved facts are observable, not independently selectable.

Raw Hands networking is also explicit:

```ts
network: {
  hands: { mode: "none" }
}
```

The only modes are `none` and `public_internet`. Hands names the untrusted
tool-execution side of a session; it is not a selectable execution host.
`public_internet` permits direct public networking. Managed model access and
other hosted service channels are separate from that raw-network choice.

## Composition and registered resources

Files, skills, tools, instructions, and MCP servers are overwrite-by-name
workspace resources. Each exact, case-sensitive name has one current value and
one monotonic revision. `set()` returns `created`, `replaced`, or `unchanged`;
there is no public copy, publish, archive, restore, or old-value read surface.

A session request names registered resources and secret metadata:

```ts
const session = await aex.sessions.create({
  model: "openai/gpt-5",
  registered: {
    files: ["repository-context"],
    instructions: ["review-policy"]
  },
  credentials: {
    secrets: [{ name: "github-token" }]
  }
});
```

Admission resolves those names into the session's effective configuration.
Later registry overwrites do not silently rewrite an admitted session. Secret
values are write-only; reads expose metadata, never plaintext.

Large registered values use disposable, checksummed upload staging. Registry
reads and writes return content descriptors rather than inline bytes or upload
identities.

## Persisted and live files

A session has two distinct file-read surfaces:

- `session.files.persisted` reads the latest explicitly persisted root without
  waking compute;
- `session.files.live` addresses the exact retained workspace generation and
  may wake it when the caller selects `wake:"retained"`.

Neither surface substitutes for the other. Persisted reads expose no historical
revision browser. `persist()` is the only operation that moves selected live
workspace state into the durable file root.

File and telemetry downloads mint short-lived bearer grants. The grant binds
the authorized object or generation, byte range, length, and immutable
whole-object hash. The SDK and CLI coordinate large ranges and verify the
declared evidence before completing a download.

## Telemetry and usage

Events, logs, spans, metrics, traces, gaps, and combined telemetry share a typed
filter and cursor model. They can be queried at session or workspace scope.
Streams expose records plus explicit gap, cursor, and rotation frames.

Large extracts are explicit telemetry-export operations. A ready export has a
separate short-lived download grant; querying observations never creates an
export.

Usage is a typed regional resource. Account and billing clients expose the
bootstrap resources that aggregate those facts without changing the regional
execution contract.

## Repository boundary

This Apache-2.0 repository owns:

- strict public schemas, identifiers, routes, errors, and scopes in
  `api/schemas`, and the OpenAPI documents, JSON Schema and Rust contract
  generated from them into `api/generated`, `crates/aex-wire`, and
  `packages/wire`;
- the service, runtime and tooling crates under `crates/`, `services/`,
  `runtimes/`, `workers/` and `tools/`;
- the TypeScript SDK in `packages/sdk`;
- the standalone native CLI in `tools/aex-cli`;
- public documentation and blackbox user tests.

Hosted authentication, scheduling, execution, persistence, billing,
observability, the dashboard, and the Terraform modules under `infra/` are also
built here, but they are implementation. They may change without changing the
public API when the strict wire behavior remains the same, and nothing in them
is a public contract.

The generation direction is one way: strict schemas produce the OpenAPI
documents, JSON Schema, Rust contract, and TypeScript wire binding, and
`cargo run -p aex-contract-gen -- check` prevents those views from drifting.

## Design rules

- One strict behavior contract; no customer-selected execution implementation.
- One explicit message admission creates one durable run.
- One current value per registered name.
- One explicit persistence operation; no hidden file capture.
- One typed observation model across query, stream, and export.
- One durable operation model for long-running mutations.
