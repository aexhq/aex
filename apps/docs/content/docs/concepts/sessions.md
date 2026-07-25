---
title: Sessions
description: Resumable threads, explicit runs, and committed checkpoints.
icon: Play
---

A session is a resumable thread. Its `status` describes whether that thread can
progress, not whether the previous run succeeded. Typical resumable states are
`running`, `idle`, `suspended`, `awaiting_approval`, and recoverable `error`.
The previous run verdict is available as `lastRun.outcome` and on its terminal
RUN event.

```ts
const session = await aex.sessions.create({ model });

const run = session.messages.send("Write the report and save it as a file.");
for await (const event of run) console.log(event.type);
const result = await run.finished();

console.log(result.status); // succeeded | failed | timed_out | cancelled | interrupted
console.log(result.session.status); // usually idle after a successful run
```

`finished()` resolves only after `RUN_FINISHED` or `RUN_ERROR`. A
`RUN_FINISHED` is the consistency barrier: session state, usage, checkpoint,
and checkpoint-backed files are committed before it is emitted. This release does not
expose a separate pre-checkpoint "brain idle" wait. The committed session
projection must identify that exact run in `lastRun`; an idle projection with a
missing or older `lastRun` is treated as inconsistent and `finished()` fails.

The three common namespaces are stable properties:

```ts
const messages = await session.messages.list();
const snapshot = await session.files.list();

for await (const event of session.events.iterate()) {
  console.log(event.type);
}

console.log(snapshot.revision.checkpointId);
console.log(snapshot.files);
```

Reopen a durable session with `aex.sessions.open(id)`. Use a stable
`idempotencyKey` for create and message mutations that your application may
repeat. Reads and explicitly idempotent mutations receive bounded transport
retries; a user run is never replayed as a whole by the SDK.

`aex.start(...)` is the one-shot create, send, and finish convenience. It
returns the same five-value run outcome and committed file snapshot.
