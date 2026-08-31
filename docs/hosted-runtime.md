# Hosted runtime

Aex is a downstream hosted composition of Brain. It does not add a second session protocol.

```text
client → Aex identity proxy → Brain Server task
                              ├─ EFS-backed disk journal
                              ├─ Wasmtime Loophost workers
                              ├─ remote model gateway
                              └─ Environment directory → Environment adapter → Tool runtime
```

Brain's public executable is independently runnable with a disk journal and in-memory current
context. The first hosted release intentionally runs one stateful Brain replica on EFS. The runtime
has explicit journal and session-ownership ports so a later placement service can shard sessions
across tasks, but the local SQLite adapter is not presented as a multi-writer database. The Aex
control service and stateless Environment adapters may scale independently.

An Environment binding is sealed during session creation. Every lifecycle and Tool command carries
the complete binding, attachment, operation ID, and canonical request digest. Any adapter task can
therefore reconstruct the same logical route; process-local connection caches are never authority.
Several sessions may attach to one shared Environment. Because the complete sealed binding travels
with every command, that identity remains routable when session placement later spans Brain tasks.

Brain journals every external intent before dispatch and its terminal result before the next
Agentloop activation. Stable operation IDs make redelivery detectable, but do not claim exactly-once
effects. An ambiguous remote outcome remains ambiguous.

Clients read session events by journal cursor and may forward them into their own queues. Brain does
not own that integration, its consumer cursor, or an at-least-once delivery guarantee. Logs, traces,
metrics, and live event projections use Brain Telemetry's bounded in-memory queue. Telemetry retries
briefly, emits a best-effort terminal-drop record after retry exhaustion, and drops under sustained
pressure; it cannot block a session or grow without bound.

The Aex control service authenticates public account and session keys, records which account owns a
session, and forwards the neutral Brain HTTP surface. It does not reinterpret Agentloop, model,
Tool, Environment, or journal contracts.

Production images are immutable and non-root. CI separately proves Rust and TypeScript contracts,
the real Linux Wasmtime worker, the Environment runtime, and container health before a release is
eligible for deployment.
