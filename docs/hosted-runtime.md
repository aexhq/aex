# Hosted runtime

Aex is a downstream hosted composition of Brain. It does not add a second session protocol.

```text
client → Aex identity proxy → Brain Server task
                              ├─ EFS-backed disk journal
                              ├─ brain-sessions: session ownership and lifecycle
                              ├─ brain-env: Wasmtime worker process pool
                              ├─ model providers
                              ├─ resident application hosts
                              └─ Environment adapter → remote Tool runtime
```

Brain's public executable is independently runnable with one canonical local-disk journal. The
first hosted release intentionally runs one Brain writer on EFS. Execution is released at turn
end by default; startup and transcript reads do not activate saved sessions. The store is behind an
interface so an external implementation can follow later; the MVP does not claim multi-writer
durability. The Aex control service and stateless Environment adapters may scale independently.

Agentloop and Tool placement is explicit in the session request. Native Agentloops and Tools are
WebAssembly Components instantiated in fresh Stores across a bounded pool of worker processes; hostEnv Tools receive commands over a
scoped SSE connection; remote Tools execute through their Environment adapter.

Brain commits an effect's start Event before dispatch and commits its terminal Event before the
next Agentloop activation. Brain does not retry model, Tool, or Environment effects. An ambiguous
remote outcome remains ambiguous so the Agentloop and model can decide what to do.

Clients read current transcripts through the ownership-checked `/v1/sessions/{id}/transcript` route,
even while execution is suspended. Callers choose Environment lifetime; providers implement
lifecycle operations and enforce physical resource ceilings. A persisted declaration does not
restore a lost browser or sandbox. Each Tool may have several authorized named placements, and
model presentation does not widen their permissions.

Clients read session Events by sequence and may persist or forward them into their own systems.
The journal is the complete history; the model transcript is derived from it. Operational logs are
separate and are never shown to the model.

The Aex control service authenticates public account and session keys, records which account owns a
session, and forwards Brain's session and extension-admission surface. Resident-host registration
uses the Aex API key; subsequent command, result, and Event requests carry the scoped host token
through to Brain. Environments are session-owned declarations; there is no standalone Environment
resource API. Aex does not reinterpret Agentloop, model, Tool, Environment, or journal contracts.

Production images are immutable and non-root. CI separately proves Rust and TypeScript contracts,
the real Linux Wasmtime worker, the Environment runtime, and container health before a release is
eligible for deployment.
