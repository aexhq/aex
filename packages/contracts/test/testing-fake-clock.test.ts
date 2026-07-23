import { describe, expect, it } from "bun:test";
import { createFakeTimers, withFakeClock } from "../src/testing.js";

describe("withFakeClock", () => {
  it("swaps globalThis timers and Date for the callback and restores them after", () => {
    const realSetTimeout = globalThis.setTimeout;
    const realDate = globalThis.Date;
    withFakeClock(() => {
      expect(globalThis.setTimeout).not.toBe(realSetTimeout);
      expect(globalThis.Date).not.toBe(realDate);
    });
    expect(globalThis.setTimeout).toBe(realSetTimeout);
    expect(globalThis.Date).toBe(realDate);
  });

  it("restores the real timers even when the callback throws", () => {
    const realSetTimeout = globalThis.setTimeout;
    expect(() =>
      withFakeClock(() => {
        throw new Error("boom");
      })
    ).toThrow("boom");
    expect(globalThis.setTimeout).toBe(realSetTimeout);
  });

  it("returns the callback's return value", () => {
    expect(withFakeClock(() => 42)).toBe(42);
  });

  it("rejects async callbacks loudly (sync fake-timer usage only)", () => {
    const realSetTimeout = globalThis.setTimeout;
    expect(() => withFakeClock(() => Promise.resolve())).toThrow(TypeError);
    expect(globalThis.setTimeout).toBe(realSetTimeout);
  });

  it("rejects nested installation loudly", () => {
    withFakeClock(() => {
      expect(() => withFakeClock(() => undefined)).toThrow(/already installed/);
    });
  });

  it("advance() fires due timeouts in due-time order, passing extra args", () => {
    withFakeClock((clock) => {
      const fired: string[] = [];
      setTimeout((suffix: string) => fired.push(`late-${suffix}`), 20, "x");
      setTimeout(() => fired.push("early"), 5);
      clock.advance(4);
      expect(fired).toEqual([]);
      clock.advance(1);
      expect(fired).toEqual(["early"]);
      clock.advance(15);
      expect(fired).toEqual(["early", "late-x"]);
    });
  });

  it("fires equal-deadline timers in registration order", () => {
    withFakeClock((clock) => {
      const fired: number[] = [];
      setTimeout(() => fired.push(1), 10);
      setTimeout(() => fired.push(2), 10);
      clock.advance(10);
      expect(fired).toEqual([1, 2]);
    });
  });

  it("runs timers scheduled during advance when they fall inside the window", () => {
    withFakeClock((clock) => {
      const fired: string[] = [];
      setTimeout(() => {
        fired.push("outer");
        setTimeout(() => fired.push("inner"), 5);
      }, 5);
      clock.advance(10);
      expect(fired).toEqual(["outer", "inner"]);
    });
  });

  it("clearTimeout cancels a pending timer", () => {
    withFakeClock((clock) => {
      const fired: string[] = [];
      const handle = setTimeout(() => fired.push("nope"), 5);
      clearTimeout(handle);
      clock.advance(10);
      expect(fired).toEqual([]);
      expect(clock.pendingTimerCount()).toBe(0);
    });
  });

  it("supports intervals: repeats until cleared", () => {
    withFakeClock((clock) => {
      let count = 0;
      const handle = setInterval(() => {
        count += 1;
      }, 10);
      clock.advance(35);
      expect(count).toBe(3);
      clearInterval(handle);
      clock.advance(50);
      expect(count).toBe(3);
    });
  });

  it("advances Date.now() and the zero-arg Date constructor with the clock", () => {
    withFakeClock((clock) => {
      const start = Date.now();
      clock.advance(1_500);
      expect(Date.now()).toBe(start + 1_500);
      expect(new Date().getTime()).toBe(start + 1_500);
      expect(new Date(0).getTime()).toBe(0);
    });
  });

  it("setSystemTime() moves now()/Date.now() without firing timers", () => {
    withFakeClock((clock) => {
      let fired = false;
      setTimeout(() => {
        fired = true;
      }, 10);
      clock.setSystemTime(clock.now() + 100_000);
      expect(fired).toBe(false);
      expect(Date.now()).toBe(clock.now());
      expect(clock.pendingTimerCount()).toBe(1);
    });
  });

  it("runAll() drains pending timeouts and throws on runaway interval graphs", () => {
    withFakeClock((clock) => {
      const fired: number[] = [];
      setTimeout(() => fired.push(1), 5);
      setTimeout(() => fired.push(2), 50);
      clock.runAll();
      expect(fired).toEqual([1, 2]);
      expect(clock.pendingTimerCount()).toBe(0);

      setInterval(() => undefined, 1);
      expect(() => clock.runAll(100)).toThrow(/runAll/);
    });
  });

  it("timer handles expose chainable no-op ref/unref for Node compatibility", () => {
    withFakeClock(() => {
      const handle = setTimeout(() => undefined, 5) as unknown as {
        ref(): unknown;
        unref(): unknown;
        hasRef(): boolean;
      };
      expect(handle.unref()).toBe(handle);
      expect(handle.ref()).toBe(handle);
      expect(handle.hasRef()).toBe(true);
    });
  });
});

describe("createFakeTimers", () => {
  it("schedules on the returned port without touching globalThis timers", () => {
    const realSetTimeout = globalThis.setTimeout;
    const clock = createFakeTimers();
    let fired = false;
    clock.setTimeout(() => {
      fired = true;
    }, 5);
    expect(globalThis.setTimeout).toBe(realSetTimeout);
    expect(fired).toBe(false);
    clock.advance(5);
    expect(fired).toBe(true);
  });

  it("advance() fires due timers in due-time order and honors clearTimeout", () => {
    const clock = createFakeTimers();
    const fired: string[] = [];
    clock.setTimeout(() => fired.push("late"), 20);
    clock.setTimeout(() => fired.push("early"), 5);
    const cancelled = clock.setTimeout(() => fired.push("never"), 10);
    clock.clearTimeout(cancelled);
    clock.advance(20);
    expect(fired).toEqual(["early", "late"]);
    expect(clock.pendingTimerCount()).toBe(0);
  });

  it("supports intervals: repeats until cleared", () => {
    const clock = createFakeTimers();
    let count = 0;
    const handle = clock.setInterval(() => {
      count += 1;
    }, 10);
    clock.advance(35);
    expect(count).toBe(3);
    clock.clearInterval(handle);
    clock.advance(50);
    expect(count).toBe(3);
  });

  it("clamps negative delays to zero like the host timers", () => {
    const clock = createFakeTimers();
    let fired = false;
    clock.setTimeout(() => {
      fired = true;
    }, -25);
    clock.advance(0);
    expect(fired).toBe(true);
  });

  it("advanceAsync() fires follow-up timers scheduled by promise continuations inside the window", async () => {
    // The exact semantics vi.advanceTimersByTimeAsync provided: a timer fires,
    // its promise continuation runs, schedules another timer, and that timer
    // still fires within the same advance window.
    const clock = createFakeTimers();
    const fired: string[] = [];
    clock.setTimeout(() => {
      fired.push("first");
      void Promise.resolve().then(() => {
        clock.setTimeout(() => fired.push("second"), 5);
      });
    }, 5);
    await clock.advanceAsync(10);
    expect(fired).toEqual(["first", "second"]);
  });

  it("advanceAsync() steps awaits chained across timer-resolved promises", async () => {
    const clock = createFakeTimers();
    const order: string[] = [];
    const sleep = (ms: number): Promise<void> =>
      new Promise<void>((resolve) => {
        clock.setTimeout(resolve, ms);
      });
    void (async () => {
      await sleep(10);
      order.push("after-first-sleep");
      await sleep(10);
      order.push("after-second-sleep");
    })();
    await clock.advanceAsync(20);
    expect(order).toEqual(["after-first-sleep", "after-second-sleep"]);
  });

  it("advanceAsync(0) drains already-settled promise chains before returning", async () => {
    const clock = createFakeTimers();
    let settled = false;
    void Promise.resolve()
      .then(() => Promise.resolve())
      .then(() => {
        settled = true;
      });
    await clock.advanceAsync(0);
    expect(settled).toBe(true);
  });

  it("advanceAsync() leaves timers beyond the window pending", async () => {
    const clock = createFakeTimers();
    let fired = false;
    clock.setTimeout(() => {
      fired = true;
    }, 11);
    await clock.advanceAsync(10);
    expect(fired).toBe(false);
    expect(clock.pendingTimerCount()).toBe(1);
    await clock.advanceAsync(1);
    expect(fired).toBe(true);
  });

  it("advanceAsync() rejects negative or non-finite advances", async () => {
    const clock = createFakeTimers();
    await expect(clock.advanceAsync(-1)).rejects.toThrow(TypeError);
    await expect(clock.advanceAsync(Number.NaN)).rejects.toThrow(TypeError);
  });

  it("keeps now()/setSystemTime/runAll from the FakeClock face", () => {
    const clock = createFakeTimers(1_000);
    expect(clock.now()).toBe(1_000);
    const fired: number[] = [];
    clock.setTimeout(() => fired.push(clock.now()), 50);
    clock.runAll();
    expect(fired).toEqual([1_050]);
    clock.setSystemTime(500_000);
    expect(clock.now()).toBe(500_000);
  });
});
