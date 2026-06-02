---
title: Events
---

# Events

antpath runs agent sessions on Goose Managed. Runs are **non-blocking**: the managed runtime advances the agent while antpath observes lifecycle state, maps runtime output into one event shape, and persists every captured event. The SDK and CLI observe the durable event timeline from antpath — there is no in-process tool-approval hook.

## Two ways to consume events

```ts
// Pull a snapshot of every event captured so far.
const events = await client.events(runId);
```

```ts
// Stream the RunEvent snapshot shape: yields each event once, stops when the
// run reaches a terminal status. Backed by polling the antpath events endpoint.
for await (const event of client.stream(runId, { intervalMs: 1000 })) {
  if (event.type === "agent.message") {
    // ...
  }
}
```

For the canonical event envelope, use the coordinator WebSocket stream:

```ts
for await (const event of client.streamEnvelopes(runId, { from: 0 })) {
  console.log(event.sequence, event.type, event.source);
}
```

`streamEnvelopes()` uses a short-lived ticket minted by the hosted API, then subscribes directly to the per-run coordinator. Subscribe means read-from-cursor plus tail: reconnects resume from the last sequence.

The CLI mirrors the same surface:

```bash
antpath events <run-id> --api-token … [--antpath-url …]                      # snapshot
antpath events <run-id> --follow [--timeout 8m] --api-token … [--antpath-url …]  # stream until terminal
antpath wait   <run-id> [--timeout 8m] [--interval 2s] --api-token …          # block, print final run
```

`antpath wait` is the host mirror of `client.wait(runId)` / `client.waitForRun(runId)`:
it polls until the run reaches a terminal status and prints the final `Run`
record. Exit `0` when the run `succeeded`, `1` for any other terminal status,
and `3` when `--timeout` elapses first (a `--timeout` on `events --follow` /
`run --follow` uses the same exit-`3` convention). Durations accept `ms`/`s`/`m`/`h`
suffixes or a bare millisecond integer.

Both surfaces observe the same events. A subscriber attached after `submitRun()` returns replays the events it missed, then continues live.

## Event shape

Events are typed as the discriminated `RunEvent` union for compatibility and as the versioned coordinator envelope for live consumers. antpath records raw runtime/provider payloads **after** secret redaction and structural sanitization, so the bytes you see never contain the provider key, MCP credentials, or proxy bearer that were supplied to `submitRun`.

## Typed helpers

The package exports conservative type guards that narrow normalized antpath event envelopes:

```ts
import {
  isRunStarted,
  isRunFinished,
  isRunError,
  isRunTerminal,
  isTextMessage,
  isToolCallStart,
  isToolCallResult,
  isCustom,
  isLog,
  isEventChannel
} from "antpath";
```

They narrow only the discriminant. Payload field shapes stay `unknown` until callers parse them, and provider-specific payloads remain behind the normalized envelope.
