/**
 * Runner-agnostic fake timers.
 *
 * Two faces over one scheduling engine:
 *
 * - `withFakeClock(fn)` swaps `globalThis` `setTimeout`/`clearTimeout`/
 *   `setInterval`/`clearInterval` AND the `Date` constructor (unlike bun's fake
 *   timers, which leave `Date` real) for the duration of the synchronous
 *   callback, restoring them in a `finally`. Sync-only by design: an async
 *   callback would escape the swapped window, so a thenable return is rejected
 *   loudly.
 * - `createFakeTimers()` returns an injectable timer host (a `FakeTimers`)
 *   that touches no globals — the deterministic double for subjects that take
 *   a timer port (e.g. the event-stream client's `timers` option). Because
 *   globals stay real, its async `advanceAsync(ms)` can interleave real
 *   event-loop turns between fired timers, reproducing the semantics of
 *   vitest's `vi.advanceTimersByTimeAsync`: continuations of promises settled
 *   by one fired timer run before the next timer fires, and follow-up timers
 *   they schedule inside the window still fire within the same advance.
 *
 * Time only moves when the test calls `advance(ms)`/`advanceAsync(ms)`; due
 * timers fire in due-time order (ties break by registration order).
 */

export interface FakeClock {
  /** Current fake epoch-ms. */
  now(): number;
  /** Move the clock without firing timers (pending deadlines keep their absolute due times). */
  setSystemTime(epochMs: number): void;
  /** Advance fake time by `ms`, synchronously firing every timer that falls due. */
  advance(ms: number): void;
  /**
   * Fire pending timers (advancing to each due time) until none remain.
   * Throws after `maxFires` fires (default 10_000) — interval graphs never drain.
   */
  runAll(maxFires?: number): void;
  /** Number of currently scheduled timers. */
  pendingTimerCount(): number;
}

/**
 * Injectable fake timer host: the four host timer functions driven by a
 * {@link FakeClock}, plus the async advance. Structurally satisfies any
 * `setTimeout`/`clearTimeout`/`setInterval`/`clearInterval` port (handles are
 * opaque), so tests pass it straight into a subject's timer seam.
 */
export interface FakeTimers extends FakeClock {
  setTimeout(callback: () => void, delayMs?: number): unknown;
  clearTimeout(handle: unknown): void;
  setInterval(callback: () => void, delayMs?: number): unknown;
  clearInterval(handle: unknown): void;
  /**
   * Advance fake time by `ms` with `vi.advanceTimersByTimeAsync` semantics:
   * before each due timer fires (and once after the window), yield a real
   * event-loop turn so every settled promise chain runs to exhaustion —
   * timers those continuations schedule inside the window fire too.
   */
  advanceAsync(ms: number): Promise<void>;
}

interface ScheduledTimer {
  readonly id: number;
  readonly callback: (...args: unknown[]) => void;
  readonly args: readonly unknown[];
  readonly intervalMs: number | undefined;
  due: number;
}

/** Node-compatible opaque handle: truthy, chainable no-op ref/unref. */
class FakeTimerHandle {
  constructor(readonly id: number) {}
  ref(): this {
    return this;
  }
  unref(): this {
    return this;
  }
  hasRef(): boolean {
    return true;
  }
  [Symbol.toPrimitive](): number {
    return this.id;
  }
}

interface TimerEngine {
  readonly clock: FakeClock;
  schedule(
    callback: (...args: unknown[]) => void,
    delayMs: unknown,
    args: readonly unknown[],
    intervalMs: number | undefined
  ): FakeTimerHandle;
  unschedule(handle: unknown): void;
  nextDue(): ScheduledTimer | undefined;
  /** Advance the clock to the timer's due time and fire it. */
  fireNext(timer: ScheduledTimer): void;
  setNow(epochMs: number): void;
}

/** The real host scheduler, captured at module load so global swaps cannot reach it. */
const hostSetTimeout = globalThis.setTimeout;

/** One real event-loop turn: every already-settled promise chain runs to exhaustion first. */
const drainEventLoop = (): Promise<void> =>
  new Promise<void>((resolve) => {
    hostSetTimeout(resolve, 0);
  });

function createTimerEngine(initialNowMs: number): TimerEngine {
  let now = initialNowMs;
  let nextId = 1;
  const timers = new Map<number, ScheduledTimer>();

  const schedule = (
    callback: (...args: unknown[]) => void,
    delayMs: unknown,
    args: readonly unknown[],
    intervalMs: number | undefined
  ): FakeTimerHandle => {
    if (typeof callback !== "function") {
      throw new TypeError("fake clock timers require a function callback");
    }
    const delay = Math.max(0, Number(delayMs) || 0);
    const id = nextId++;
    timers.set(id, { id, callback, args, intervalMs, due: now + delay });
    return new FakeTimerHandle(id);
  };

  const unschedule = (handle: unknown): void => {
    if (handle instanceof FakeTimerHandle) timers.delete(handle.id);
    else if (typeof handle === "number") timers.delete(handle);
  };

  const nextDue = (): ScheduledTimer | undefined => {
    let best: ScheduledTimer | undefined;
    for (const timer of timers.values()) {
      if (best === undefined || timer.due < best.due || (timer.due === best.due && timer.id < best.id)) {
        best = timer;
      }
    }
    return best;
  };

  const fireNext = (timer: ScheduledTimer): void => {
    now = Math.max(now, timer.due);
    if (timer.intervalMs === undefined) {
      timers.delete(timer.id);
    } else {
      timer.due += Math.max(1, timer.intervalMs);
    }
    timer.callback(...timer.args);
  };

  const clock: FakeClock = {
    now: () => now,
    setSystemTime: (epochMs: number) => {
      if (!Number.isFinite(epochMs)) {
        throw new TypeError("setSystemTime requires a finite epoch-ms value");
      }
      now = epochMs;
    },
    advance: (ms: number) => {
      if (!Number.isFinite(ms) || ms < 0) {
        throw new TypeError("advance requires a non-negative finite ms value");
      }
      const target = now + ms;
      for (;;) {
        const timer = nextDue();
        if (timer === undefined || timer.due > target) break;
        fireNext(timer);
      }
      now = target;
    },
    runAll: (maxFires = 10_000) => {
      for (let fires = 0; ; fires += 1) {
        const timer = nextDue();
        if (timer === undefined) return;
        if (fires >= maxFires) {
          throw new Error(
            `runAll exceeded ${maxFires} timer fires with ${timers.size} still pending (interval graph?)`
          );
        }
        fireNext(timer);
      }
    },
    pendingTimerCount: () => timers.size
  };

  return {
    clock,
    schedule,
    unschedule,
    nextDue,
    fireNext,
    setNow: (epochMs: number) => {
      now = epochMs;
    }
  };
}

/** Injectable fake timer host over a fresh engine; touches no globals. */
export function createFakeTimers(initialNowMs: number = Date.now()): FakeTimers {
  const engine = createTimerEngine(initialNowMs);
  return {
    ...engine.clock,
    setTimeout: (callback, delayMs) => engine.schedule(callback, delayMs, [], undefined),
    clearTimeout: (handle) => engine.unschedule(handle),
    setInterval: (callback, delayMs) => engine.schedule(callback, delayMs, [], Math.max(1, Number(delayMs) || 0)),
    clearInterval: (handle) => engine.unschedule(handle),
    advanceAsync: async (ms: number): Promise<void> => {
      if (!Number.isFinite(ms) || ms < 0) {
        throw new TypeError("advanceAsync requires a non-negative finite ms value");
      }
      const target = engine.clock.now() + ms;
      for (;;) {
        await drainEventLoop();
        const timer = engine.nextDue();
        if (timer === undefined || timer.due > target) break;
        engine.fireNext(timer);
      }
      engine.setNow(target);
      await drainEventLoop();
    }
  };
}

let installed = false;

export function withFakeClock<T>(fn: (clock: FakeClock) => T): T {
  if (installed) {
    throw new Error("withFakeClock: a fake clock is already installed (no nesting)");
  }

  const RealDate = globalThis.Date;
  const realSetTimeout = globalThis.setTimeout;
  const realClearTimeout = globalThis.clearTimeout;
  const realSetInterval = globalThis.setInterval;
  const realClearInterval = globalThis.clearInterval;

  const engine = createTimerEngine(RealDate.now());

  class FakeDate extends RealDate {
    constructor(...args: readonly unknown[]) {
      if (args.length === 0) {
        super(engine.clock.now());
      } else {
        super(...(args as ConstructorParameters<DateConstructor>));
      }
    }

    static override now(): number {
      return engine.clock.now();
    }
  }

  installed = true;
  globalThis.setTimeout = ((callback: (...args: unknown[]) => void, delayMs?: unknown, ...args: unknown[]) =>
    engine.schedule(callback, delayMs, args, undefined)) as never;
  globalThis.clearTimeout = engine.unschedule as never;
  globalThis.setInterval = ((callback: (...args: unknown[]) => void, delayMs?: unknown, ...args: unknown[]) =>
    engine.schedule(callback, delayMs, args, Math.max(1, Number(delayMs) || 0))) as never;
  globalThis.clearInterval = engine.unschedule as never;
  globalThis.Date = FakeDate as DateConstructor;

  try {
    const result = fn(engine.clock);
    if (result !== null && typeof result === "object" && "then" in (result as object)) {
      throw new TypeError(
        "withFakeClock is sync-only: the callback returned a thenable, which would outlive the fake clock"
      );
    }
    return result;
  } finally {
    installed = false;
    globalThis.setTimeout = realSetTimeout;
    globalThis.clearTimeout = realClearTimeout;
    globalThis.setInterval = realSetInterval;
    globalThis.clearInterval = realClearInterval;
    globalThis.Date = RealDate;
  }
}
