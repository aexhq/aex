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

A session's reads and streams are grouped under accessor sub-resources:
`session.events()` owns the event timeline, `session.messages()` owns the decoded
assistant text, and `session.outputs()` owns the captured files. Reach a verb by
chaining it off the accessor.

```ts
// Pull a snapshot of every event captured so far.
const events = await session.events().list();
```

```ts
// Stream the RunEvent snapshot shape: yields each event once, stops when the
// session parks. Backed by polling the aex events endpoint.
for await (const event of session.events().stream({ intervalMs: 1000 })) {
  if (event.type === "TEXT_MESSAGE_CONTENT") {
    // ...
  }
}
```

For the canonical event envelope, use the coordinator WebSocket stream:

```ts
for await (const event of session.events().streamEnvelopes({ from: 0 })) {
  console.log(event.sequence, event.type, event.source);
}
```

`session.events().streamEnvelopes()` uses a short-lived ticket minted by the hosted API, then subscribes directly to the per-session coordinator. Subscribe means read-from-cursor plus tail: reconnects resume from the last sequence.

## Assistant text

To collect just the agent's assistant messages, use the `messages()` accessor —
`list()` returns every decoded `AssistantTextEntry` oldest-first, and
`last()`/`first()` return one entry (or `undefined` when empty). Read `.text`
for the string:

```ts
const lastText = (await session.messages().last())?.text;
```

Prefer `session.messages().list()` or the collected `result.messages` /
`result.text` fields for assistant text. Low-level event helpers remain exported
for callers that build custom collectors.

The CLI mirrors the same surface:

```bash
aex events  <session-id> --api-key … [--aex-url …]                      # snapshot (polling)
aex events  <session-id> --follow [--timeout 8m] --api-key … [--aex-url …]  # stream until the session parks (polling)
aex tail    <session-id> [--json] [--filter <type|source>] [--logs] [--settle] [--timeout 8m] --api-key …  # live, human-readable, over the WS envelope stream
aex inspect <session-id> [--json] [--filter <type|source>] [--logs] [--timeout 8m] --api-key …             # one-shot full timeline + jump-to-failure + cost/usage
aex wait    <session-id> [--timeout 8m] [--interval 2s] --api-key …          # block, print final session
```

`aex tail` and `aex inspect` consume the same coordinator WebSocket envelope
stream as `session.events().streamEnvelopes()` (replay-from-cursor + tail +
exactly-once resume), so they are the low-latency equivalents of
`events --follow`'s polling. `--json` is the raw-NDJSON escape hatch; `--filter`
keeps only the named AG-UI types (`TEXT_MESSAGE_CONTENT`, `TOOL_CALL_START`, …)
or sources (`agent`/`runtime`/…); a `RUN_ERROR` is surfaced as a jump-to-failure
line. `aex inspect` adds a header, a settle-consistent full timeline, and a
cost/usage footer. Both exit `0` parked cleanly / `1` error park / `3` timeout.
They need a global `WebSocket` (Bun or Node ≥ 22).

`aex wait` is the host mirror of `session.wait()`:
it polls until the session parks and prints the final `Session` record. Exit `0`
when the session parked cleanly (`idle`/`suspended`), `1` for any other park
(`error` / a non-clean terminal status), and `3` when `--timeout` elapses first
(a `--timeout` on `events --follow` / `run --follow` uses the same exit-`3`
convention). Durations accept `ms`/`s`/`m`/`h` suffixes or a bare millisecond integer.

Both surfaces observe the same events. A subscriber attached after a session
message is accepted replays the events it missed, then continues live.

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

Two families of events can end a turn's stream, and which one you see depends
on how the turn ends:

- **AG-UI terminals** — `RUN_FINISHED` / `RUN_ERROR`. These are *render-complete*
  signals emitted by the agent stream itself. On the managed plane a normal
  session turn usually does **not** emit `RUN_FINISHED`: the session *parks*
  instead (see below). Expect `RUN_ERROR` on stream-level failures, and treat
  `RUN_FINISHED` — when it does appear — as a low-latency "stop the spinner"
  hint, not a read-consistency barrier.
- **`aex.session.*` park terminals** — `CUSTOM` events named `aex.session.idle`,
  `aex.session.suspended`, or `aex.session.error`. On the managed plane these
  are what actually end a turn: the session parks with the matching status, and
  by the time the park event is broadcast the session record has already
  reached that status. This is the terminal you should expect from a managed
  run's event stream.

The SDK's helpers cover both families so you never have to switch on the plane:

- `isRunTerminal(event)` — true for the AG-UI `RUN_FINISHED` / `RUN_ERROR` pair.
- `isRunSettled(event)` — true for the `aex.run.settled` settle barrier **and**
  for any `aex.session.*` park terminal. The managed plane does not broadcast a
  separate `aex.run.settled` barrier — the park event plays that role — so
  `isRunSettled` is the one guard that reliably means "this stream is done and
  the record is authoritative".

To read the authoritative status consistently, use one of:

```ts
// Session record path: send a turn, then wait for the session to park.
const session = await aex.openSession(config);
await session.send("Continue the task.").done();
const record = await session.wait(); // the parked session record
```

```ts
// Live events AND a settle-consistent end: the iterator ends on the settle
// barrier OR the aex.session.* park terminal, whichever the plane emits —
// so when it ends, the session record is already parked/terminal.
for await (const event of session.events().streamEnvelopes({ settleConsistent: true })) {
  // render events live…
}
const settled = await aex.sessions.get(session.id); // parked/terminal here
```

`settleConsistent: true` makes the iterator end exactly when `isRunSettled(event)`
first fires; on a raw stream, apply `isRunSettled(event)` yourself. What it
guarantees: when the stream ends, a subsequent `aex.sessions.get(id)` reads a
parked/terminal status and `session.outputs().list()` is complete. Outputs are
uploaded before the terminal is broadcast, so they are readable the moment the
stream ends.

## Temporary event archive links

For terminal runs, `session.events().archiveLink(options?)` returns a temporary direct URL to `events.jsonl`, the same redacted customer-visible event export used by `session.events().download()`.

```ts
const link = await session.events().archiveLink({ expiresIn: "1h" });
const response = await fetch(link.url);
const jsonl = await response.text();
```

`expiresIn` accepts seconds or `"15m"`, `"1h"`, or `"1d"`; the default is `"1h"`. The URL is a reusable bearer URL until it expires, so treat it like a short-lived secret. Internal runtime, host, and provider diagnostics are not included in this export.

## Event shape

Events are typed as the discriminated `RunEvent` union for compatibility and as the versioned coordinator envelope for live consumers. aex records raw runtime/provider payloads **after** secret redaction and structural sanitization, so the bytes you see never contain provider keys, MCP credentials, or runtime secrets supplied when the session was opened.

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
  isEventChannel
} from "@aexhq/sdk";
```

All guards test the `type` discriminant at runtime. `isTextMessage`,
`isToolCallStart`, `isToolCallResult`, and `isRunFinished` operate on the loose
`RunEvent` snapshot (`session.events().list()` / `RunResult.events`) and additionally NARROW
`event.data` to the fields that event type carries — e.g. inside
`if (isTextMessage(e))`, `e.data.text` is typed `string`. The lifecycle/channel
guards (`isRunStarted`, `isRunError`, `isCustom`, `isLog`, …) operate on the
coordinator envelope and narrow only the discriminant. Use `result.text` or
`session.messages.all()` when you need assistant text without inspecting the
event stream directly.
