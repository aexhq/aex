import { describe, expect, it } from "vitest";
import { withFakeClock } from "../src/testing.js";

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
