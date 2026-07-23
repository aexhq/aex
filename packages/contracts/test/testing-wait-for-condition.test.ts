import { describe, expect, it } from "bun:test";
import { waitForCondition } from "../src/testing.js";

describe("waitForCondition", () => {
  it("resolves with the first truthy predicate value", async () => {
    let calls = 0;
    const value = await waitForCondition(
      () => {
        calls += 1;
        return calls >= 3 ? { ready: calls } : undefined;
      },
      { deadline: 2_000, interval: 1 }
    );
    expect(value).toEqual({ ready: 3 });
    expect(calls).toBe(3);
  });

  it("supports async predicates", async () => {
    let calls = 0;
    const value = await waitForCondition(
      async () => {
        calls += 1;
        return calls >= 2 ? "done" : null;
      },
      { deadline: 2_000, interval: 1 }
    );
    expect(value).toBe("done");
  });

  it("rejects after the deadline with label, attempt count, and diagnostic output", async () => {
    await expect(
      waitForCondition(() => false, {
        deadline: 30,
        interval: 5,
        label: "job should exit",
        diagnostic: () => "pid=17 alive=true"
      })
    ).rejects.toThrow(/job should exit[\s\S]*attempts[\s\S]*pid=17 alive=true/);
  });

  it("makes a final predicate attempt at the deadline before rejecting", async () => {
    let calls = 0;
    await expect(
      waitForCondition(
        () => {
          calls += 1;
          return false;
        },
        { deadline: 20, interval: 8 }
      )
    ).rejects.toThrow(/timed out/);
    expect(calls).toBeGreaterThanOrEqual(2);
  });

  it("propagates predicate exceptions immediately without retrying", async () => {
    let calls = 0;
    await expect(
      waitForCondition(
        () => {
          calls += 1;
          throw new Error("probe exploded");
        },
        { deadline: 2_000, interval: 1 }
      )
    ).rejects.toThrow("probe exploded");
    expect(calls).toBe(1);
  });

  it("awaits diagnostics that are async", async () => {
    await expect(
      waitForCondition(() => undefined, {
        deadline: 10,
        interval: 5,
        diagnostic: async () => "async diagnostic detail"
      })
    ).rejects.toThrow(/async diagnostic detail/);
  });

  it("rejects invalid deadlines and intervals loudly", async () => {
    await expect(waitForCondition(() => true, { deadline: 0 })).rejects.toThrow(TypeError);
    await expect(waitForCondition(() => true, { deadline: Number.NaN })).rejects.toThrow(TypeError);
    await expect(
      waitForCondition(() => true, { deadline: 100, interval: 0 })
    ).rejects.toThrow(TypeError);
  });
});
