/**
 * Unit tests for the bounded-concurrency prepare-pass helper (§2.5): order is
 * preserved regardless of completion order, the in-flight cap is respected, and
 * a rejection propagates.
 */
import { describe, expect, it } from "vitest";
import { mapWithConcurrency } from "../../src/client.js";

/** A deferred promise so a test can control settle order. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("mapWithConcurrency", () => {
  it("preserves order even when later items settle first", async () => {
    const gates = [deferred<void>(), deferred<void>(), deferred<void>()];
    const p = mapWithConcurrency([0, 1, 2], 5, async (item, i) => {
      await gates[i]!.promise;
      return item * 10;
    });
    // Settle out of order: index 2, then 0, then 1.
    gates[2]!.resolve();
    gates[0]!.resolve();
    gates[1]!.resolve();
    expect(await p).toEqual([0, 10, 20]);
  });

  it("respects the concurrency cap (never more than `limit` in flight)", async () => {
    let inFlight = 0;
    let maxInFlight = 0;
    const limit = 3;
    const items = Array.from({ length: 12 }, (_, i) => i);
    const out = await mapWithConcurrency(items, limit, async (item) => {
      inFlight++;
      maxInFlight = Math.max(maxInFlight, inFlight);
      await new Promise((r) => setTimeout(r, 1));
      inFlight--;
      return item;
    });
    expect(out).toEqual(items);
    expect(maxInFlight).toBeLessThanOrEqual(limit);
    expect(maxInFlight).toBe(limit); // 12 items / cap 3 → the cap is actually reached
  });

  it("propagates a rejection from any worker", async () => {
    await expect(
      mapWithConcurrency([1, 2, 3], 2, async (item) => {
        if (item === 2) throw new Error("boom on 2");
        return item;
      })
    ).rejects.toThrow(/boom on 2/);
  });

  it("returns an empty array for empty input (no workers spawned)", async () => {
    let calls = 0;
    const out = await mapWithConcurrency([], 5, async (item) => {
      calls++;
      return item;
    });
    expect(out).toEqual([]);
    expect(calls).toBe(0);
  });

  it("caps workers at the item count when limit exceeds it", async () => {
    let inFlight = 0;
    let maxInFlight = 0;
    const out = await mapWithConcurrency([1, 2], 10, async (item) => {
      inFlight++;
      maxInFlight = Math.max(maxInFlight, inFlight);
      await new Promise((r) => setTimeout(r, 1));
      inFlight--;
      return item;
    });
    expect(out).toEqual([1, 2]);
    expect(maxInFlight).toBeLessThanOrEqual(2);
  });
});
