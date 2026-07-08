/**
 * Client-side consumer of the event coordinator's WebSocket stream.
 *
 * One mechanism for catch-up + resume + live: subscribe = read-from-cursor +
 * tail. The consumer opens a WS to the coordinator with a connection ticket,
 * replays from its cursor, and yields {@link AexEvent}s as they arrive.
 * On a transport drop it reconnects with backoff and resumes from the last
 * sequence it saw — `from = lastSeq + 1` — so delivery is exactly-once across
 * reconnects (no gap, no duplicate). It stops on a terminal event, on abort,
 * or when the caller breaks the iterator.
 *
 * A silently half-open socket (no close/error, no frames) is the dangerous
 * case: the read loop would block forever and MISS a terminal that was already
 * persisted server-side. So the client sends a post-open replay trigger plus a
 * tiny keep-alive ping the coordinator answers with a matching pong, and runs an idle
 * watchdog: if no frame arrives within {@link CoordinatorStreamOptions.idleTimeoutMs},
 * the socket is treated as dead and reconnected — resume-from-cursor then
 * replays the terminal.
 *
 * Filtering and projection are the client's concern (the wire carries the
 * whole run): compose {@link filterStream} with the envelope guards, and
 * {@link mapStream} with {@link toAGUI}, on top of this stream.
 *
 * The WebSocket is injectable so the SDK/CLI use the global `WebSocket`
 * (Bun and Node 22+ ship it; no dependency) and tests drive a fake.
 */

import type { AexEvent } from "./event-envelope.js";
import { isSessionParked } from "./event-envelope.js";

/** The slice of the WHATWG WebSocket this client depends on. */
export interface WebSocketLike {
  close(code?: number, reason?: string): void;
  addEventListener(type: "open" | "message" | "close" | "error", listener: (ev: { data?: unknown }) => void): void;
  /** Send a keep-alive ping. Optional: a transport without it just relies on real events to reset the watchdog. */
  send?(data: string): void;
}
export type WebSocketFactory = (url: string) => WebSocketLike;

export interface CoordinatorStreamOptions {
  /** Base subscribe URL, e.g. `wss://coordinator/runs/<id>/subscribe`. */
  readonly wsUrl: string;
  /** Starting cursor: events with `sequence >= from` are delivered. Default 0 (from start). */
  readonly from?: number;
  /** Mint/refresh a short-lived connection ticket (called before each connect). */
  readonly fetchTicket: () => Promise<string>;
  readonly signal?: AbortSignal;
  /** Injected WebSocket constructor; defaults to the global `WebSocket`. */
  readonly webSocketFactory?: WebSocketFactory;
  /** Reconnect ceiling (default: unlimited until terminal/abort). */
  readonly maxReconnects?: number;
  /** Backoff between reconnect attempts (default 500 ms). */
  readonly reconnectDelayMs?: number;
  /**
   * Predicate that decides which event ENDS the stream. Default: the AG-UI
   * terminal events (RUN_FINISHED / RUN_ERROR) — the render-complete UX signal.
   * Pass {@link isRunSettled} for a settle-consistent stream that keeps reading
   * PAST RUN_FINISHED until the post-mirror barrier, so the iterator only ends
   * once a subsequent `getRun` is guaranteed terminal.
   */
  readonly isTerminal?: (event: AexEvent) => boolean;
  /**
   * Half-open watchdog window. If no frame (a real event OR a keep-alive pong)
   * arrives within this many ms, the socket is treated as dead and reconnected
   * (resume from cursor). Default 45s. Set 0 to disable.
   */
  readonly idleTimeoutMs?: number;
  /**
   * Client keep-alive ping cadence. The client sends {@link COORDINATOR_PING},
   * which the coordinator answers with a matching pong, so
   * a legitimately quiet run keeps the socket measurably alive and does not trip
   * the watchdog. Default 15s. Set 0 to disable (then only real events reset the
   * watchdog → quiet runs may reconnect).
   */
  readonly pingIntervalMs?: number;
  /**
   * Event-quiet recheck window. A pong proves the SOCKET is alive, not the
   * delivery pipeline behind it — a server-side subscription that died (reaped
   * connection row, wedged fan-out) keeps answering pings while never delivering
   * another event, so the idle watchdog alone would hang one frame short of the
   * terminal forever. If no REAL event frame arrives within this many ms the
   * client silently reconnects (resume from cursor) — the replay-on-connect path
   * reads the event store directly, so a dead subscription self-heals. Default
   * 90s. Set 0 to disable.
   */
  readonly eventQuietRecheckMs?: number;
  /**
   * A WebSocket can receive a live terminal frame before the post-open replay
   * has backfilled lower-sequence frames. If a terminal event arrives with a
   * sequence gap, hold it briefly so replayed content can be delivered first.
   * Default 1s. Set 0 to disable.
   */
  readonly terminalDrainGraceMs?: number;
}

// The default terminal predicate ends the stream on the AG-UI terminals AND on
// the managed runtime's CUSTOM session-park terminal (aex.session.idle/.error/
// .suspended). A managed one-shot run parks instead of emitting RUN_FINISHED, so
// WITHOUT the session-park arm a `streamEnvelopes()` over a finished managed run
// never sees a terminal and hangs on the idle watchdog forever.
const isTerminalType = (e: AexEvent): boolean =>
  e.type === "RUN_FINISHED" || e.type === "RUN_ERROR" || isSessionParked(e);

/**
 * Keep-alive ping the client sends; the coordinator answers it with the matching
 * pong in its WebSocket message handler. Must stay byte-identical to
 * the coordinator's pair (aex-platform `packages/shared/src/event-stream-client.ts`).
 */
const COORDINATOR_PING = "aex:ping";
/** Post-open replay request; $connect cannot safely PostToConnection before the handshake completes. */
const COORDINATOR_REPLAY = JSON.stringify({ action: "replay" });
/** Default half-open watchdog window — 3× the ping cadence, so 2 pongs can be lost. */
const DEFAULT_IDLE_TIMEOUT_MS = 45_000;
/** Default client keep-alive ping cadence. */
const DEFAULT_PING_INTERVAL_MS = 15_000;
/** Default event-quiet recheck window (a silent reconnect, so the cost of a false positive is small). */
const DEFAULT_EVENT_QUIET_RECHECK_MS = 90_000;
/** Default drain window for terminal-before-replay races. */
const DEFAULT_TERMINAL_DRAIN_GRACE_MS = 1_000;

export async function* streamCoordinatorEvents(
  opts: CoordinatorStreamOptions
): AsyncGenerator<AexEvent, void, void> {
  const makeWs =
    opts.webSocketFactory ?? ((url: string) => new WebSocket(url) as unknown as WebSocketLike);
  const isTerminal = opts.isTerminal ?? isTerminalType;
  const reconnectDelayMs = opts.reconnectDelayMs ?? 500;
  const maxReconnects = opts.maxReconnects ?? Number.POSITIVE_INFINITY;
  const idleTimeoutMs = opts.idleTimeoutMs ?? DEFAULT_IDLE_TIMEOUT_MS;
  const pingIntervalMs = opts.pingIntervalMs ?? DEFAULT_PING_INTERVAL_MS;
  const eventQuietRecheckMs = opts.eventQuietRecheckMs ?? DEFAULT_EVENT_QUIET_RECHECK_MS;
  const terminalDrainGraceMs = opts.terminalDrainGraceMs ?? DEFAULT_TERMINAL_DRAIN_GRACE_MS;
  let cursor = (opts.from ?? 0) - 1;
  let attempts = 0;
  let done = false;

  while (!done && !opts.signal?.aborted) {
    const ticket = await opts.fetchTicket();
    const url = new URL(opts.wsUrl);
    url.searchParams.set("ticket", ticket);
    url.searchParams.set("from", String(cursor + 1));
    const ws = makeWs(url.toString());

    const pending: AexEvent[] = [];
    const seenSequences = new Set<number>();
    let terminalDrainUntil = 0;
    let closed = false;
    let disconnectReason = "";
    let resolveNext: (() => void) | null = null;
    const wake = (): void => {
      if (resolveNext) {
        const r = resolveNext;
        resolveNext = null;
        r();
      }
    };
    const waitForWake = (timeoutMs?: number): Promise<void> =>
      new Promise<void>((resolve) => {
        let timer: ReturnType<typeof setTimeout> | null = null;
        const finish = (): void => {
          if (timer !== null) {
            clearTimeout(timer);
            timer = null;
          }
          if (resolveNext === finish) resolveNext = null;
          resolve();
        };
        resolveNext = finish;
        if (typeof timeoutMs === "number") timer = setTimeout(finish, timeoutMs);
      });
    const sortPending = (): void => {
      pending.sort((a, b) => a.sequence - b.sequence);
    };
    const terminalWaitMs = (): number | null => {
      if (pending.length === 0) return null;
      sortPending();
      const next = pending[0]!;
      if (!isTerminal(next) || terminalDrainUntil <= 0) return null;
      const remaining = terminalDrainUntil - Date.now();
      return remaining > 0 ? remaining : null;
    };

    let idleTimer: ReturnType<typeof setTimeout> | null = null;
    let pingTimer: ReturnType<typeof setInterval> | null = null;
    let quietTimer: ReturnType<typeof setTimeout> | null = null;
    const stopTimers = (): void => {
      if (idleTimer !== null) {
        clearTimeout(idleTimer);
        idleTimer = null;
      }
      if (pingTimer !== null) {
        clearInterval(pingTimer);
        pingTimer = null;
      }
      if (quietTimer !== null) {
        clearTimeout(quietTimer);
        quietTimer = null;
      }
    };
    // Re-arm on every inbound frame. On expiry the socket is presumed half-open
    // → close it and let the loop fall through to reconnect (resume from cursor).
    const armIdle = (): void => {
      if (idleTimeoutMs <= 0) return;
      if (idleTimer !== null) clearTimeout(idleTimer);
      idleTimer = setTimeout(() => {
        idleTimer = null;
        if (closed) return;
        closed = true;
        disconnectReason = "idle";
        closeQuietly(ws);
        wake();
      }, idleTimeoutMs);
    };
    // Re-arm only on REAL event frames. On expiry: silent reconnect (resume from
    // cursor) — self-heals a dead server-side subscription a pong can't expose.
    const armQuiet = (): void => {
      if (eventQuietRecheckMs <= 0) return;
      if (quietTimer !== null) clearTimeout(quietTimer);
      quietTimer = setTimeout(() => {
        quietTimer = null;
        if (closed) return;
        closed = true;
        disconnectReason = "quiet_recheck";
        closeQuietly(ws);
        wake();
      }, eventQuietRecheckMs);
    };

    ws.addEventListener("open", () => {
      armIdle();
      if (typeof ws.send === "function") {
        try {
          ws.send(COORDINATOR_REPLAY);
        } catch {
          // socket not open / send unsupported — the DDB-stream replay kicker and
          // quiet-reconnect path still cover replay.
        }
      }
      if (pingIntervalMs > 0 && typeof ws.send === "function") {
        pingTimer = setInterval(() => {
          try {
            ws.send!(COORDINATOR_PING);
          } catch {
            // socket not open / send unsupported — the idle watchdog still covers it
          }
        }, pingIntervalMs);
      }
    });
    ws.addEventListener("message", (ev) => {
      armIdle();
      const data = typeof ev.data === "string" ? ev.data : "";
      if (!data) return;
      try {
        const evt = JSON.parse(data) as AexEvent;
        if (typeof evt.sequence === "number") {
          armQuiet(); // a real event frame proves the delivery pipeline, not just the socket
          if (evt.sequence > cursor && !seenSequences.has(evt.sequence)) {
            if (
              terminalDrainGraceMs > 0 &&
              isTerminal(evt) &&
              evt.sequence > cursor + 1
            ) {
              terminalDrainUntil = Math.max(terminalDrainUntil, Date.now() + terminalDrainGraceMs);
            }
            seenSequences.add(evt.sequence);
            pending.push(evt);
            wake();
          }
        }
      } catch {
        // ignore a non-JSON frame
      }
    });
    ws.addEventListener("close", (ev) => {
      stopTimers();
      // Don't clobber a reason already decided (idle/abort closes the socket too).
      if (!closed) {
        closed = true;
        const code = (ev as { code?: number } | undefined)?.code;
        disconnectReason = `close${typeof code === "number" ? ` code=${code}` : ""}`;
      }
      wake();
    });
    ws.addEventListener("error", () => {
      stopTimers();
      if (!closed) {
        closed = true;
        disconnectReason = "error";
      }
      wake();
    });

    const onAbort = (): void => {
      closeQuietly(ws);
      closed = true;
      stopTimers();
      wake();
    };
    opts.signal?.addEventListener("abort", onAbort, { once: true });

    // Arm immediately: a connect that never reaches "open" (and fakes that
    // never emit it) must still time out rather than hang forever. The quiet
    // recheck arms here too so a socket fed only by pongs still recycles.
    armIdle();
    armQuiet();

    try {
      while (true) {
        while (pending.length > 0) {
          if (terminalWaitMs() !== null) break;
          sortPending();
          const evt = pending.shift()!;
          if (evt.sequence <= cursor) continue;
          cursor = evt.sequence;
          yield evt;
          if (isTerminal(evt)) done = true;
        }
        if (done || opts.signal?.aborted) {
          closeQuietly(ws);
          break;
        }
        const waitMs = terminalWaitMs();
        if (closed) break; // transport ended → fall through to reconnect
        await waitForWake(waitMs ?? undefined);
      }
    } finally {
      stopTimers();
      opts.signal?.removeEventListener("abort", onAbort);
      // A caller that `break`s the iterator triggers the generator's `return()`,
      // which lands here with the socket still OPEN (the yield was suspended, no
      // terminal/abort/transport-close ran). Close it so an early break never
      // leaks a live WebSocket. Idempotent: the terminal/abort/reconnect paths
      // have already closed it, and closeQuietly swallows a double close.
      closeQuietly(ws);
    }

    if (done || opts.signal?.aborted) return;
    attempts += 1;
    // Don't let the stream die silently — a give-up before the terminal event
    // is exactly the case a caller needs to see (it looks like a clean end
    // otherwise). Reconnects are rare, so the warn volume is bounded.
    if (attempts > maxReconnects) {
      console.warn(
        `[aex] event stream gave up after ${maxReconnects} reconnect attempt(s) (last: ${disconnectReason || "unknown"}); ended before a terminal event at seq ${cursor + 1}`
      );
      return;
    }
    // The quiet recheck is a routine self-heal on a legitimately quiet stream —
    // reconnecting silently keeps a long tool call from spamming the console.
    if (disconnectReason !== "quiet_recheck") {
      console.warn(
        `[aex] event stream disconnected (${disconnectReason || "unknown"}); reconnecting attempt ${attempts} from seq ${cursor + 1}`
      );
    }
    await sleep(reconnectDelayMs, opts.signal);
  }
}

/** Async-iterable filter — keep only events matching the predicate. */
export async function* filterStream<T>(
  stream: AsyncIterable<T>,
  predicate: (event: T) => boolean
): AsyncGenerator<T, void, void> {
  for await (const event of stream) {
    if (predicate(event)) yield event;
  }
}

/** Async-iterable map — project each event (e.g. with `toAGUI`). */
export async function* mapStream<T, U>(
  stream: AsyncIterable<T>,
  project: (event: T) => U
): AsyncGenerator<U, void, void> {
  for await (const event of stream) {
    yield project(event);
  }
}

function closeQuietly(ws: WebSocketLike): void {
  try {
    ws.close();
  } catch {
    // already closed
  }
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise<void>((resolve) => {
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener(
      "abort",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true }
    );
  });
}
