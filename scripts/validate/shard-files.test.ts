import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { buildFileMatrix, collectTestFiles, excludeFiles, loadDurations, lptPartition } from "../../apps/user-tests/scripts/shard-files.mjs";
import type { ShardBin } from "../../apps/user-tests/scripts/shard-files.mjs";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const userTestsRoot = resolve(repoRoot, "apps/user-tests");

function durationsOf(entries: Record<string, number>): Map<string, number> {
  return new Map(Object.entries(entries));
}

function expectBin(bins: ShardBin[], index: number): ShardBin {
  const bin = bins[index];
  if (!bin) throw new Error(`expected shard bin ${index} to exist`);
  return bin;
}

describe("shard-files duration-balanced bin packing", () => {
  it("builds one complete, unique matrix entry per collected file", () => {
    const files = collectTestFiles(userTestsRoot);
    const matrix = buildFileMatrix(files);

    expect(matrix).toHaveLength(files.length);
    expect(matrix.map((entry) => entry.file)).toEqual(files);
    expect(new Set(matrix.map((entry) => entry.file)).size).toBe(files.length);
    expect(matrix.map((entry) => entry.shard)).toEqual(files.map((_, index) => index + 1));
    expect(matrix.every((entry) => entry.count === files.length)).toBe(true);
  });

  it("partitions the real collected suite completely and deterministically", () => {
    const files = collectTestFiles(userTestsRoot);
    const durations = loadDurations();

    expect(files.length).toBeGreaterThanOrEqual(50);
    // Excluded explicit gates never leak into the default sweep.
    expect(files).not.toContain("test/live/edge-admission-gates.user.test.ts");
    expect(files).not.toContain("test/live/live-sdk-heavy-session.test.ts");
    expect(files).not.toContain("test/live/live-api-fuzz.test.ts");
    expect(files).not.toContain("test/live/live-sdk-tool-capability-fuzz.test.ts");
    expect(files.some((f) => f.startsWith("test/live/providers/"))).toBe(false);
    // Provider-specific suites (Anthropic BYOK, doubao, ...) are non-gating:
    // they live under test/live/providers/ and never enter the 50 shards.
    expect(files).not.toContain("test/live/live-sdk-anthropic-managed.test.ts");
    expect(files).not.toContain("test/live/providers/live-sdk-anthropic-managed.test.ts");

    const bins = lptPartition(files, durations, 50);
    const all = bins.flatMap((bin) => bin.files);
    // Completeness + disjointness: every collected file in exactly one shard.
    expect([...all].sort()).toEqual([...files].sort());
    expect(new Set(all).size).toBe(files.length);
    for (const bin of bins) expect(bin.files.length).toBeGreaterThan(0);

    // Deterministic: same inputs => identical partition.
    const again = lptPartition(files, durations, 50);
    expect(again.map((b) => b.files)).toEqual(bins.map((b) => b.files));
  });

  it("balances by duration, not file count", () => {
    const files = ["a.ts", "b.ts", "c.ts", "d.ts"];
    const durations = durationsOf({ "a.ts": 100, "b.ts": 1, "c.ts": 1, "d.ts": 1 });
    const bins = lptPartition(files, durations, 2);
    expect(expectBin(bins, 0).files).toEqual(["a.ts"]);
    expect(expectBin(bins, 1).files).toEqual(["b.ts", "c.ts", "d.ts"]);
  });

  it("assigns unknown files the median of recorded durations", () => {
    const files = ["known-big.ts", "known-mid.ts", "known-small.ts", "unknown.ts"];
    const durations = durationsOf({ "known-big.ts": 100, "known-mid.ts": 10, "known-small.ts": 1 });
    // median = 10 => unknown.ts weighs 10 and pairs with known-small, not known-big.
    const bins = lptPartition(files, durations, 2);
    expect(expectBin(bins, 0).files).toEqual(["known-big.ts"]);
    const secondBin = expectBin(bins, 1);
    expect(secondBin.files).toEqual(["known-mid.ts", "known-small.ts", "unknown.ts"]);
    expect(secondBin.seconds).toBe(21);
  });

  it("fails loudly when a shard would be empty", () => {
    const durations = durationsOf({ "a.ts": 1 });
    expect(() => lptPartition(["a.ts", "b.ts"], durations, 3)).toThrow(/empty/);
    expect(() => lptPartition([], durations, 1)).toThrow(/no test files/);
    expect(() => lptPartition(["a.ts"], durations, 0)).toThrow(/invalid shard count/);
  });

  it("applies environment-specific file exclusions before partitioning", () => {
    const files = collectTestFiles(userTestsRoot);
    const filtered = excludeFiles(files, ["test/live/live-default-base-url.test.ts"]);
    const durations = loadDurations();

    expect(files).toContain("test/live/live-default-base-url.test.ts");
    expect(filtered).not.toContain("test/live/live-default-base-url.test.ts");
    expect(filtered.length).toBe(files.length - 1);

    const bins = lptPartition(filtered, durations, 50);
    expect(bins).toHaveLength(50);
    for (const bin of bins) expect(bin.files.length).toBeGreaterThan(0);
    expect(bins.flatMap((bin) => bin.files)).not.toContain("test/live/live-default-base-url.test.ts");
  });

  it("fails loudly when asked to exclude a non-collected file", () => {
    expect(() => excludeFiles(["a.ts"], ["missing.ts"])).toThrow(/excluded file/);
  });

  it("keeps the checked-in durations file keyed to real, collectable test files", () => {
    const files = new Set(collectTestFiles(userTestsRoot));
    const durations = loadDurations();
    for (const key of durations.keys()) {
      expect(files.has(key), `shard-durations.json entry no longer collected: ${key}`).toBe(true);
    }
  });
});
