---
title: Events
---

# Events

aex runs agent sessions on the managed runtime. Sessions are **non-blocking**:
the managed runtime advances the agent while aex observes lifecycle state, maps
runtime output into one event shape, and persists every captured event. The SDK
and CLI observe the durable event timeline from aex — there is no in-process
tool-approval hook.

## Two ways to consume events

```ts
// Pull a snapshot of every event captured so far.
const events = await aex.events(runId);
```

```ts
// Stream the RunEvent snapshot shape: yields each event once, stops when the
// run reaches a terminal status. Backed by polling the aex events endpoint.
for await (const event of aex.stream(runId, { intervalMs: 1000 })) {
  if (event.type === "TEXT_MESSAGE_CONTENT") {
    // ...
  }
}
```

For the canonical event envelope, use the coordinator WebSocket stream:

```ts
for await (const event of aex.streamEnvelopes(runId, { from: 0 })) {
  console.log(event.sequence, event.type, event.source);
}
```

`streamEnvelopes()` uses a short-lived ticket minted by the hosted API, then subscribes directly to the per-run coordinator. Subscribe means read-from-cursor plus tail: reconnects resume from the last sequence.

The CLI mirrors the same surface:

```bash
aex events  <run-id> --api-token … [--aex-url …]                      # snapshot (polling)
aex events  <run-id> --follow [--timeout 8m] --api-token … [--aex-url …]  # stream until terminal (polling)
aex tail    <run-id> [--json] [--filter <type|source>] [--logs] [--settle] [--timeout 8m] --api-token …  # live, human-readable, over the WS envelope stream
aex inspect <run-id> [--json] [--filter <type|source>] [--logs] [--timeout 8m] --api-token …             # one-shot full timeline + jump-to-failure + cost/usage
aex wait    <run-id> [--timeout 8m] [--interval 2s] --api-token …          # block, print final run
```

`aex tail` and `aex inspect` consume the same coordinator WebSocket envelope
stream as `streamEnvelopes()` (replay-from-cursor + tail + exactly-once resume),
so they are the low-latency equivalents of `events --follow`'s polling. `--json`
is the raw-NDJSON escape hatch; `--filter` keeps only the named AG-UI types
(`TEXT_MESSAGE_CONTENT`, `TOOL_CALL_START`, …) or sources (`agent`/`runtime`/…);
a `RUN_ERROR` is surfaced as a jump-to-failure line. `aex inspect` adds a header,
a settle-consistent full timeline, and a cost/usage footer. Both exit `0`
succeeded / `1` other terminal / `3` timeout. They need a global `WebSocket`
(Bun or Node ≥ 22).

`aex wait` is the host mirror of `aex.wait(runId)` / `aex.waitForRun(runId)`:
it polls until the run reaches a terminal status and prints the final `Run`
record. Exit `0` when the run `succeeded`, `1` for any other terminal status,
and `3` when `--timeout` elapses first (a `--timeout` on `events --follow` /
`run --follow` uses the same exit-`3` convention). Durations accept `ms`/`s`/`m`/`h`
suffixes or a bare millisecond integer.

Both surfaces observe the same events. A subscriber attached after `submit()` or
a session message is accepted replays the events it missed, then continues live.

## Session turn events

The canonical SDK session surface stops a turn on session lifecycle events, not
terminal run events:

```ts
const session = await aex.openSession(config);
const turn = session.send("Continue the task.");

for await (const event of turn) {
  console.log(event.sequence, event.type);
}

const result = await turn.done(); // status is usually "idle"
```

The turn stream ends when it sees `aex.session.idle`, `aex.session.suspended`, or
`aex.session.error` for that turn. `aex.run(config)` is a convenience wrapper
over the same flow: it opens a session, sends `message` once, and returns the
collected session turn. The returned `runId` is the session id.

## Terminal events vs. the run record

The low-level `submit()` run path emits a terminal **event** — `RUN_FINISHED`
(success) or `RUN_ERROR` — when
the agent's stream ends. This is an AG-UI *render-complete* signal: the runner
emits it **before** aex commits the authoritative run record, so a `getRun(runId)`
issued the instant you observe `RUN_FINISHED` can still read `status: "running"`
for a moment. Treat the terminal event as the lowest-latency "stop the spinner"
signal — **not** a read-consistency barrier.

Two facts make this easy to work with:

- **Outputs are already durable at the terminal event.** The runner uploads every
  output before it emits the terminal event, and `listOutputs(runId)` / downloads
  read object storage directly — so the moment you see `RUN_FINISHED` the outputs
  are complete and readable.
- **The run _record_ settles a beat later.** To read the authoritative status
  consistently, don't key off the terminal event — use one of:

```ts
// Low-level run record path: submit + wait.
const runId = await aex.submit(runConfig);
const sameRun = await aex.waitForRun(runId); // or wait on an already-submitted run for the bare Run record
```

```ts
// Live events AND a settle-consistent end: the iterator keeps reading past
// RUN_FINISHED until the post-mirror barrier, so the record is terminal when it ends.
for await (const event of aex.streamEnvelopes(runId, { settleConsistent: true })) {
  // render events live…
}
const run = await aex.getRun(runId); // guaranteed terminal here
```

Under the hood the coordinator broadcasts one `aex.run.settled` CUSTOM event as a
run's last stream event, immediately after the durable record commits.
`settleConsistent` ends the stream on it; on a raw stream, detect it with
`isRunSettled(event)`.

## Temporary event archive links

For terminal runs, `eventArchiveLink(runId, options?)` returns a temporary direct URL to `events.jsonl`, the same redacted customer-visible event export used by `downloadEvents(runId)`.

```ts
const link = await aex.eventArchiveLink(runId, { expiresIn: "1h" });
const response = await fetch(link.url);
const jsonl = await response.text();
```

`expiresIn` accepts seconds or `"15m"`, `"1h"`, or `"1d"`; the default is `"1h"`. The URL is a reusable bearer URL until it expires, so treat it like a short-lived secret. Internal runtime, host, and provider diagnostics are not included in this export.

## Event shape

Events are typed as the discriminated `RunEvent` union for compatibility and as the versioned coordinator envelope for live consumers. aex records raw runtime/provider payloads **after** secret redaction and structural sanitization, so the bytes you see never contain the provider key, MCP credentials, or proxy bearer that were supplied to `submit`.

## Typed helpers

The package exports conservative type guards over run events:

```ts
import {
  isRunStarted,
  isRunFinished,
  isRunError,
  isRunTerminal,
  isRunSettled,
  isTextMessage,
  isToolCallStart,
  isToolCallResult,
  isCustom,
  isLog,
  isEventChannel,
  textOf
} from "@aexhq/sdk";
```

All guards test the `type` discriminant at runtime. `isTextMessage`,
`isToolCallStart`, `isToolCallResult`, and `isRunFinished` operate on the loose
`RunEvent` snapshot (`listEvents` / `RunResult.events`) and additionally NARROW
`event.data` to the fields that event type carries — e.g. inside
`if (isTextMessage(e))`, `e.data.text` is typed `string`. The lifecycle/channel
guards (`isRunStarted`, `isRunError`, `isCustom`, `isLog`, …) operate on the
coordinator envelope and narrow only the discriminant. `textOf(events)` returns
the run's final assistant text concatenated from the `TEXT_MESSAGE_CONTENT`
blocks.
