import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";
import {
  buildFileMatrix,
  collectTestFiles,
  excludeFiles,
  lptPartition
} from "../../apps/user-tests/scripts/shard-files.mjs";
import type { ShardBin } from "../../apps/user-tests/scripts/shard-files.mjs";

const repoRoot = resolve(import.meta.dirname, "..", "..");
const userTestsRoot = resolve(repoRoot, "apps", "user-tests");

function durations(entries: Record<string, number>): Map<string, number> {
  return new Map(Object.entries(entries));
}

function binAt(bins: ShardBin[], index: number): ShardBin {
  const bin = bins[index];
  if (!bin) throw new Error(`expected bin ${index}`);
  return bin;
}

describe("single-architecture live user-test sharding", () => {
  it("covers every live test exactly once without runtime or capability cells", () => {
    const files = collectTestFiles(userTestsRoot);
    const matrix = buildFileMatrix(files, {
      shards: Math.min(2, files.length),
      durations: new Map(files.map((file) => [file, 1]))
    });
    const covered = matrix.flatMap((entry) => entry.files);

    expect([...covered].sort()).toEqual([...files].sort());
    expect(new Set(covered).size).toBe(files.length);
    expect(matrix.map((entry) => entry.shard)).toEqual(matrix.map((_, index) => index + 1));
    expect(matrix.every((entry) => entry.count === matrix.length)).toBe(true);
    expect(matrix.every((entry) => entry.file === entry.files.join(" "))).toBe(true);
    expect(matrix.every((entry) => entry.sessionSlots === 1)).toBe(true);
    expect(JSON.stringify(matrix)).not.toMatch(/runtime|capabilit|parity/i);
  });

  it("balances long files by duration and assigns unknown files the median", () => {
    const bins = lptPartition(
      ["long.ts", "medium.ts", "short.ts", "unknown.ts"],
      durations({ "long.ts": 100, "medium.ts": 10, "short.ts": 1 }),
      2
    );

    expect(binAt(bins, 0).files).toEqual(["long.ts"]);
    expect(binAt(bins, 1).files).toEqual(["medium.ts", "short.ts", "unknown.ts"]);
    expect(binAt(bins, 1).seconds).toBe(21);
  });

  it("fails closed on empty shards and unknown exclusions", () => {
    const weights = durations({ "a.ts": 1 });
    expect(() => lptPartition([], weights, 1)).toThrow(/no test files/);
    expect(() => lptPartition(["a.ts"], weights, 2)).toThrow(/empty/);
    expect(() => lptPartition(["a.ts"], weights, 0)).toThrow(/invalid shard count/);
    expect(() => excludeFiles(["a.ts"], ["missing.ts"])).toThrow(/not collected/);
  });
});
