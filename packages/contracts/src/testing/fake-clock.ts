/**
 * Runner-agnostic synchronous fake clock.
 *
 * `withFakeClock(fn)` swaps `globalThis` `setTimeout`/`clearTimeout`/
 * `setInterval`/`clearInterval` AND the `Date` constructor (unlike bun's fake
 * timers, which leave `Date` real) for the duration of the synchronous
 * callback, restoring them in a `finally`. Time only moves when the test calls
 * `clock.advance(ms)`; due timers fire synchronously in due-time order
 * (ties break by registration order).
 *
 * Sync-only by design: an async callback would escape the swapped window, so
 * a thenable return is rejected loudly.
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

  let now = RealDate.now();
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

  const fire = (timer: ScheduledTimer): void => {
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
        now = Math.max(now, timer.due);
        fire(timer);
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
        now = Math.max(now, timer.due);
        fire(timer);
      }
    },
    pendingTimerCount: () => timers.size
  };

  class FakeDate extends RealDate {
    constructor(...args: readonly unknown[]) {
      if (args.length === 0) {
        super(now);
      } else {
        super(...(args as ConstructorParameters<DateConstructor>));
      }
    }

    static override now(): number {
      return now;
    }
  }

  installed = true;
  globalThis.setTimeout = ((callback: (...args: unknown[]) => void, delayMs?: unknown, ...args: unknown[]) =>
    schedule(callback, delayMs, args, undefined)) as never;
  globalThis.clearTimeout = unschedule as never;
  globalThis.setInterval = ((callback: (...args: unknown[]) => void, delayMs?: unknown, ...args: unknown[]) =>
    schedule(callback, delayMs, args, Math.max(1, Number(delayMs) || 0))) as never;
  globalThis.clearInterval = unschedule as never;
  globalThis.Date = FakeDate as DateConstructor;

  try {
    const result = fn(clock);
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
