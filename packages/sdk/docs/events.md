---
title: Events
---

# Events

aex runs agent sessions on the managed runtime. Runs are **non-blocking**: the managed runtime advances the agent while aex observes lifecycle state, maps runtime output into one event shape, and persists every captured event. The SDK and CLI observe the durable event timeline from aex — there is no in-process tool-approval hook.

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
aex events <run-id> --api-token … [--aex-url …]                      # snapshot
aex events <run-id> --follow [--timeout 8m] --api-token … [--aex-url …]  # stream until terminal
aex wait   <run-id> [--timeout 8m] [--interval 2s] --api-token …          # block, print final run
```

`aex wait` is the host mirror of `aex.wait(runId)` / `aex.waitForRun(runId)`:
it polls until the run reaches a terminal status and prints the final `Run`
record. Exit `0` when the run `succeeded`, `1` for any other terminal status,
and `3` when `--timeout` elapses first (a `--timeout` on `events --follow` /
`run --follow` uses the same exit-`3` convention). Durations accept `ms`/`s`/`m`/`h`
suffixes or a bare millisecond integer.

Both surfaces observe the same events. A subscriber attached after `submitRun()` returns replays the events it missed, then continues live.

## Terminal events vs. the run record

A run emits a terminal **event** — `RUN_FINISHED` (success) or `RUN_ERROR` — when
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
// Blocking: resolves only once the RECORD is terminal (polls getRun, not the event).
const run = await aex.run(runConfig);      // submit + wait
const same = await aex.waitForRun(runId);  // or wait on an already-submitted run
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

## Event shape

Events are typed as the discriminated `RunEvent` union for compatibility and as the versioned coordinator envelope for live consumers. aex records raw runtime/provider payloads **after** secret redaction and structural sanitization, so the bytes you see never contain the provider key, MCP credentials, or proxy bearer that were supplied to `submitRun`.

## Typed helpers

The package exports conservative type guards that narrow normalized aex event envelopes:

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

They narrow only the discriminant. Payload field shapes stay `unknown` until callers parse them, and provider-specific payloads remain behind the normalized envelope.
