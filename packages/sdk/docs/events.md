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
// Stream events as guarded `AexEventView`s: yields each event once, stops when
// the session parks. Backed by polling the aex events endpoint. Every surface —
// list(), stream(), streamEnvelopes() — yields the same `AexEventView` with a
// populated `sequence` and the `is*()` guard methods.
for await (const event of session.events().stream({ intervalMs: 1000 })) {
  if (event.isTextMessage()) {
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
when the session parked cleanly (`idle`/`suspended`/`succeeded`), `1` for any
non-clean terminal outcome (`failed`/`timed_out`/`cancelled`), and `3` when
`--timeout` elapses first
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

const result = await turn.done(); // status is the terminal outcome, e.g. "succeeded"
```

The turn stream ends on the turn's park terminal: a resumable
`aex.session.idle` / `aex.session.suspended`, a terminal outcome
`aex.session.succeeded` / `.failed` / `.timed_out` / `.cancelled`, or the held
`aex.session.awaiting_approval`. (The bare `aex.session.error` park is retired —
a failed turn is `failed`, a wall-clock kill `timed_out`, a cancel `cancelled`.)
`aex.run(config)` is a convenience wrapper over the same flow: it opens a
session, sends `message` once, and returns the collected session turn. The
returned `runId` is the session id.

## Per-token streaming (`outputMode: 'stream'`)

Assistant-text granularity is controlled by `outputMode` on `openSession` / `run`:

- **`buffered`** (the default) — the assistant text arrives coalesced as one
  `TEXT_MESSAGE_CONTENT` event per message.
- **`stream`** — on a **streamable** provider you also receive per-token
  `TEXT_MESSAGE_CONTENT` **deltas** as the model produces them, and a coalesced
  final `TEXT_MESSAGE_CONTENT` (the archived twin) always follows. Streamable
  providers today are Anthropic and the OpenAI-chat-shaped providers (DeepSeek,
  OpenAI, Mistral, OpenRouter, Doubao / Doubao China).

Streaming is **capability-gated and fail-closed**: requesting `outputMode: 'stream'`
on a provider that has no streaming producer (currently `gemini`) is a typed
rejection at submission parse — **not** a silent downgrade to buffered. Choose a
streamable provider or drop back to `buffered` explicitly.

## Terminal events vs. the run record

Two families of events can end a turn's stream, and which one you see depends
on how the turn ends:

- **AG-UI terminals** — `RUN_FINISHED` / `RUN_ERROR`. These are *render-complete*
  signals emitted by the agent stream itself. On the managed plane a normal
  session turn usually does **not** emit `RUN_FINISHED`: the session *parks*
  instead (see below). Expect `RUN_ERROR` on stream-level failures, and treat
  `RUN_FINISHED` — when it does appear — as a low-latency "stop the spinner"
  hint, not a read-consistency barrier.
- **`aex.session.*` park terminals** — `CUSTOM` events carrying the turn's
  OUTCOME: `aex.session.succeeded` / `aex.session.failed` / `aex.session.timed_out`
  / `aex.session.cancelled`, or a resumable/held park `aex.session.idle` /
  `aex.session.suspended` / `aex.session.awaiting_approval`. On the managed plane
  these are what actually end a turn. The park event ends the RENDER; the settle
  commit (cost + usage + the authoritative outcome) lands shortly after — which
  is why `run()`/`done()` await settle by DEFAULT and return the outcome as
  `result.status` with `result.costUsd`/`result.usage` always populated (never a
  bare `idle`). Pass `await: 'park'` (on `run(...)` or `session.send(msg, {...})`)
  to return early at the park event when you don't need cost/usage. `result.costUsd`
  is aex runtime/storage spend and **excludes** your BYOK provider charges — price
  those from `result.usage` token counts. Ending a raw stream on the park event is
  fine for live UI; read the settled record (or the default await-settle result)
  when you need cost/usage/outcome.

Each event the stream yields carries method guards that cover both families so
you never have to switch on the plane:

- `event.isRunTerminal()` — true for the AG-UI `RUN_FINISHED` / `RUN_ERROR` pair.
- `event.isRunSettled()` — true for the `aex.run.settled` settle barrier **and**
  for any `aex.session.*` park terminal. The managed plane emits the barrier after
  settle when it is delivered to the stream; the park event remains a terminal
  fallback for streams that do not receive a later barrier. `event.isRunSettled()`
  is the one check that reliably means "this stream is done and the record is
  authoritative".

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

`settleConsistent: true` makes the iterator end exactly when `event.isRunSettled()`
first fires; on a raw stream, call `event.isRunSettled()` yourself. What it
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

Every read and stream surface yields one canonical shape — the versioned coordinator envelope, wrapped as an `AexEventView` with a populated non-optional `sequence` and the `is*()` guard methods. `sequence` is **sparse by design** (it encodes journal position, not a dense 0..N counter), so resume by cursor VALUE (`from: lastSequence`), never by integer adjacency. Tool-call START and RESULT events share a `data.id` join key, exposed as `event.toolCallId()` on those views. aex records raw runtime/provider payloads **after** secret redaction and structural sanitization, so the bytes you see never contain provider keys, MCP credentials, or runtime secrets supplied when the session was opened.

## Typed helpers

Every event the SDK yields — the turn stream (`session.send()`),
`session.events().list()`, `session.events().streamEnvelopes()`, and
`RunResult.events` — carries a type-guard **method** for each standardized event
type, so you branch on the event without importing a free function or writing a
raw `event.type === "…"` compare:

```ts
for await (const event of session.send("Continue the task.")) {
  if (event.isTextMessage()) {
    process.stdout.write(event.data.text); // `data.text` is typed `string` here
  } else if (event.isToolCallStart()) {
    console.log("tool:", event.data.name); // `data.name` is typed `string` here
  } else if (event.isRunError()) {
    console.error("run error");
  }
}
```

The full method set: `isRunStarted()`, `isRunFinished()`, `isRunError()`,
`isRunTerminal()`, `isTextMessage()`, `isToolCallStart()`, `isToolCallResult()`,
`isCustom()`, `isLog()`, `isEventChannel()`, `isRunSettled()`, and
`isFromSource(source)`. All test the `type` (or `channel`/`source`) discriminant
at runtime. `isTextMessage()`, `isToolCallStart()`, and `isToolCallResult()` are
TS type predicates: inside the guarded branch `event.data` NARROWS to that event
type's payload fields (e.g. `event.data.text` is typed `string`). Annotate a
narrowed event with the exported `TextMessageEventView` / `ToolCallStartEventView`
/ `ToolCallResultEventView` types, or a raw event with `AexEventView`. Use
`result.text` or `session.messages.all()` when you need assistant text without
inspecting the event stream directly.
