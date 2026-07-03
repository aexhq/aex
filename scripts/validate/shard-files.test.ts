import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { collectTestFiles, loadDurations, lptPartition } from "../../apps/user-tests/scripts/shard-files.mjs";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const userTestsRoot = resolve(repoRoot, "apps/user-tests");

function durationsOf(entries: Record<string, number>): Map<string, number> {
  return new Map(Object.entries(entries));
}

describe("shard-files duration-balanced bin packing", () => {
  it("partitions the real collected suite completely and deterministically", () => {
    const files = collectTestFiles(userTestsRoot) as string[];
    const durations = loadDurations() as Map<string, number>;

    expect(files.length).toBeGreaterThanOrEqual(11);
    // Excluded explicit gates never leak into the default sweep.
    expect(files).not.toContain("test/live/live-sdk-heavy-session.test.ts");
    expect(files).not.toContain("test/live/live-api-fuzz.test.ts");
    expect(files).not.toContain("test/live/live-sdk-tool-capability-fuzz.test.ts");
    expect(files.some((f) => f.startsWith("test/live/providers/"))).toBe(false);
    // Provider-specific suites (Anthropic BYOK, doubao, …) are non-gating:
    // they live under test/live/providers/ and never enter the 11 shards.
    expect(files).not.toContain("test/live/live-sdk-anthropic-managed.test.ts");
    expect(files).not.toContain("test/live/providers/live-sdk-anthropic-managed.test.ts");

    const bins = lptPartition(files, durations, 11) as Array<{ files: string[]; seconds: number }>;
    const all = bins.flatMap((bin) => bin.files);
    // Completeness + disjointness: every collected file in exactly one shard.
    expect([...all].sort()).toEqual([...files].sort());
    expect(new Set(all).size).toBe(files.length);
    for (const bin of bins) expect(bin.files.length).toBeGreaterThan(0);

    // Deterministic: same inputs => identical partition.
    const again = lptPartition(files, durations, 11) as Array<{ files: string[] }>;
    expect(again.map((b) => b.files)).toEqual(bins.map((b) => b.files));
  });

  it("balances by duration, not file count", () => {
    const files = ["a.ts", "b.ts", "c.ts", "d.ts"];
    const durations = durationsOf({ "a.ts": 100, "b.ts": 1, "c.ts": 1, "d.ts": 1 });
    const bins = lptPartition(files, durations, 2) as Array<{ files: string[]; seconds: number }>;
    expect(bins[0].files).toEqual(["a.ts"]);
    expect(bins[1].files).toEqual(["b.ts", "c.ts", "d.ts"]);
  });

  it("assigns unknown files the median of recorded durations", () => {
    const files = ["known-big.ts", "known-mid.ts", "known-small.ts", "unknown.ts"];
    const durations = durationsOf({ "known-big.ts": 100, "known-mid.ts": 10, "known-small.ts": 1 });
    // median = 10 => unknown.ts weighs 10 and pairs with known-small, not known-big.
    const bins = lptPartition(files, durations, 2) as Array<{ files: string[]; seconds: number }>;
    expect(bins[0].files).toEqual(["known-big.ts"]);
    expect(bins[1].files).toEqual(["known-mid.ts", "known-small.ts", "unknown.ts"]);
    expect(bins[1].seconds).toBe(21);
  });

  it("fails loudly when a shard would be empty", () => {
    const durations = durationsOf({ "a.ts": 1 });
    expect(() => lptPartition(["a.ts", "b.ts"], durations, 3)).toThrow(/empty/);
    expect(() => lptPartition([], durations, 1)).toThrow(/no test files/);
    expect(() => lptPartition(["a.ts"], durations, 0)).toThrow(/invalid shard count/);
  });

  it("keeps the checked-in durations file keyed to real, collectable test files", () => {
    const files = new Set(collectTestFiles(userTestsRoot) as string[]);
    const durations = loadDurations() as Map<string, number>;
    for (const key of durations.keys()) {
      expect(files.has(key), `shard-durations.json entry no longer collected: ${key}`).toBe(true);
    }
  });
});
