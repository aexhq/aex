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
 * Filtering and projection are the client's concern (the wire carries the
 * whole run): compose {@link filterStream} with the envelope guards, and
 * {@link mapStream} with {@link toAGUI}, on top of this stream.
 *
 * The WebSocket is injectable so the SDK/CLI use the Node/global `WebSocket`
 * (Node 22+ ships it; no dependency) and tests drive a fake.
 */

import type { AexEvent } from "./event-envelope.js";

/** The slice of the WHATWG WebSocket this client depends on. */
export interface WebSocketLike {
  close(code?: number, reason?: string): void;
  addEventListener(type: "open" | "message" | "close" | "error", listener: (ev: { data?: unknown }) => void): void;
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
}

const isTerminalType = (t: string): boolean => t === "RUN_FINISHED" || t === "RUN_ERROR";

export async function* streamCoordinatorEvents(
  opts: CoordinatorStreamOptions
): AsyncGenerator<AexEvent, void, void> {
  const makeWs =
    opts.webSocketFactory ?? ((url: string) => new WebSocket(url) as unknown as WebSocketLike);
  const reconnectDelayMs = opts.reconnectDelayMs ?? 500;
  const maxReconnects = opts.maxReconnects ?? Number.POSITIVE_INFINITY;
  let cursor = (opts.from ?? 0) - 1;
  let attempts = 0;
  let done = false;

  while (!done && !opts.signal?.aborted) {
    const ticket = await opts.fetchTicket();
    const url = `${opts.wsUrl}?ticket=${encodeURIComponent(ticket)}&from=${cursor + 1}`;
    const ws = makeWs(url);

    const queue: AexEvent[] = [];
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

    ws.addEventListener("message", (ev) => {
      const data = typeof ev.data === "string" ? ev.data : "";
      if (!data) return;
      try {
        const evt = JSON.parse(data) as AexEvent;
        if (typeof evt.sequence === "number" && evt.sequence > cursor) {
          queue.push(evt);
          wake();
        }
      } catch {
        // ignore a non-JSON frame
      }
    });
    ws.addEventListener("close", (ev) => {
      closed = true;
      const code = (ev as { code?: number } | undefined)?.code;
      disconnectReason = `close${typeof code === "number" ? ` code=${code}` : ""}`;
      wake();
    });
    ws.addEventListener("error", () => {
      closed = true;
      disconnectReason = "error";
      wake();
    });

    const onAbort = (): void => {
      closeQuietly(ws);
      closed = true;
      wake();
    };
    opts.signal?.addEventListener("abort", onAbort, { once: true });

    try {
      while (true) {
        while (queue.length > 0) {
          const evt = queue.shift()!;
          cursor = evt.sequence;
          yield evt;
          if (isTerminalType(evt.type)) done = true;
        }
        if (done || opts.signal?.aborted) {
          closeQuietly(ws);
          break;
        }
        if (closed) break; // transport ended → fall through to reconnect
        await new Promise<void>((resolve) => {
          resolveNext = resolve;
        });
      }
    } finally {
      opts.signal?.removeEventListener("abort", onAbort);
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
    console.warn(
      `[aex] event stream disconnected (${disconnectReason || "unknown"}); reconnecting attempt ${attempts} from seq ${cursor + 1}`
    );
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
