---
title: Events
---

# Events

Durable event reads use one `AexEventView` shape with CloudEvents identity,
monotonic `sequence`, real AG-UI `threadId`, and real `runId`.
The `AEX_EVENT_TYPES` and `AEX_EVENT_SOURCES` arrays list the events authored by
the current SDK, but the raw envelope accepts future type and source strings.
Unknown events are yielded with their original fields and data intact; `toAGUI`
projects an unknown type through AG-UI's `CUSTOM` carrier without changing the
raw event.

```ts
const session = await aex.sessions.open(sessionId);
for await (const event of session.events.iterate()) {
  if (event.isTextMessage()) console.log(event.data.text);
  if (event.isToolCallStart()) console.log(event.data.name);
}
```

`iterate()` follows cursor pages lazily and retains at most one API page. Use
`list()` only when the full history is known to fit comfortably in memory.

## Run lifecycle

The public lifecycle is:

- `RUN_STARTED`: the accepted run began.
- `RUN_FINISHED`: all run state is committed, including the checkpoint.
- `RUN_ERROR`: the run failed. It may have no checkpoint when failure happened
  before a checkpoint could be created.

Runtime execution ending is internal and is not emitted as a public lifecycle
event. Consumers observe only the committed `RUN_FINISHED` or `RUN_ERROR`
terminal boundary.

```ts
const run = session.messages.send("Continue the task.");

for await (const event of run) {
  if (event.isRunFinished()) {
    console.log(event.data.checkpoint);
  }
}

const result = await run.finished();
```

Message sends do not accept a historical `from` cursor. Their stream and
finished result are scoped to the run accepted by that send, so events from an
earlier run cannot be replayed into the current result. Use
`session.events.iterate()`, `.list()`, `.stream({ from })`, or
`.streamEnvelopes({ from })`
when intentionally reading session history.

`finished()` and `aex.start()` return the same run outcome vocabulary:
`succeeded`, `failed`, `timed_out`, `cancelled`, or `interrupted`. A run held by
suspension or approval is `interrupted`; the session lifecycle separately says
`suspended` or `awaiting_approval`. Only `succeeded` sets `result.ok` to true.

## Live stream

```ts
for await (const event of session.events.streamEnvelopes({ from: 0 })) {
  if (event.replayable === false) {
    console.log("live", event.liveSequence, event.type);
  } else {
    console.log("durable", event.sequence, event.type);
  }
}
```

`session.messages.send(...)` and `session.events.streamEnvelopes(...)` yield
`AexStreamEventView`, a discriminated union:

- Durable events have `sequence` and are replayable. They are the only events
  returned by `events.iterate()`, `events.list()`, polling streams, archives,
  and finished results.
- Provisional live events have `replayable: false`, a per-run `liveSequence`,
  `ephemeral: true`, the coordinator `receivedAt` time, and no durable
  `sequence`. They are not replayed from storage.

The SDK mints a short-lived coordinator ticket, reconnects durable events from
the last sequence after transient transport loss, deduplicates repeated live
frames by event id, and stops on a durable run terminal. The caller can tune
transport watchdogs with `idleTimeoutMs`, `pingIntervalMs`, and
`eventQuietRecheckMs`; these reconnect the event transport and never retry an
application run.

For a polling snapshot stream, use `session.events.stream()`. At invocation it
targets the session's current run, or its last completed run when idle. Older
run terminals cannot end that poll. The inclusive `from` cursor
(`sequence >= from`) filters yielded history without hiding the target run's
completion boundary:

```ts
for await (const event of session.events.stream({ from: 1024 })) {
  console.log(event.sequence, event.type);
}
```

The cursor must be a non-negative safe integer. A session with no runs returns
one bounded snapshot instead of waiting for a terminal that cannot exist. For
an explicitly bounded read, use `session.events.list()`.

## Assistant messages

Messages are a separate resource from raw events:

```ts
const messages = await session.messages.list();
console.log(messages.at(-1)?.text);
```

Messages come directly from the messages endpoint. A missing or broken route is
surfaced as an API failure rather than projected from another resource.

## Output granularity

`outputMode: "buffered"` emits coalesced assistant text. `"stream"` first emits
provisional live token deltas, then exactly one durable coalesced
`TEXT_MESSAGE_CONTENT` before the run terminal. Final `result.text`,
`result.messages`, and `result.events` use the durable coalesced event and do
not duplicate provisional deltas. Stream mode is rejected before submission
for providers that do not support it; it is never silently downgraded.

## Archives

```ts
const link = await session.events.archiveLink({ expiresIn: "1h" });
const archive = await session.events.download();
```

The event archive contains the same public events the SDK returns, byte-identical.
`archiveLink()` is a bounded convenience export. A session that exceeds the
bulk-export limits returns a typed `413` API error; an export that cannot finish
inside the server's request budget returns a typed `503` API error. The SDK does
not retry either response automatically. Traverse large histories with
`for await (const event of session.events.iterate())`; iteration follows bounded
cursor pages without retaining the whole history. `events.list()` and
`events.download()` are materializing convenience methods.
