import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
// @ts-expect-error JavaScript CI gate helper is consumed directly.
import { listJunitTestcases } from "../cicd/junit-report.mjs";
import { EDGE_CHAT_SESSION_SHARDS } from "../../apps/user-tests/test/_fixtures/edge-chat-session-manifest.js";
import {
  buildFileMatrix,
  collectTestFiles,
  declaredPeakSessionSlots,
  excludeFiles,
  loadDurations,
  lptPartition,
  LIVE_TEST_SHARD_CONFIG,
  RUNTIME_PAIRED_FILES,
  sessionSlotsForFile
} from "../../apps/user-tests/scripts/shard-files.mjs";
import type { ShardBin } from "../../apps/user-tests/scripts/shard-files.mjs";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const userTestsRoot = resolve(repoRoot, "apps/user-tests");
const SHARD_COLLECTION_PROCESS_TIMEOUT_MS = 30_000;

function allLiveTestFiles(): readonly string[] {
  const files: string[] = [];
  const excludedDirectories = new Set(LIVE_TEST_SHARD_CONFIG.excludedDirectories);
  const visit = (relativeDirectory: string): void => {
    for (const entry of readdirSync(join(userTestsRoot, relativeDirectory), { withFileTypes: true })) {
      const relativePath = `${relativeDirectory}/${entry.name}`;
      if (entry.isDirectory()) {
        if (!excludedDirectories.has(entry.name)) visit(relativePath);
      } else if (entry.isFile() && entry.name.endsWith(".test.ts")) {
        files.push(relativePath);
      }
    }
  };
  visit("test/live");
  return files.sort();
}

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
    expect(matrix.every((entry) => entry.sessionSlots === sessionSlotsForFile(entry.file))).toBe(true);
    expect(matrix.filter((entry) => entry.runtimeKind !== null).map((entry) => entry.file).sort()).toEqual(
      [...RUNTIME_PAIRED_FILES].sort()
    );
    expect(matrix.filter((entry) => entry.runtimeKind !== null).every((entry) => entry.runtimeKind === "spot_container"))
      .toBe(true);
  });

  it("fans out only runtime-aware files across the authenticated available runtime set", () => {
    const files = collectTestFiles(userTestsRoot);
    const runtimeKinds = ["container", "spot_container", "lambda"] as const;
    const matrix = buildFileMatrix(files, runtimeKinds);
    const expectedCount = files.length + RUNTIME_PAIRED_FILES.size * (runtimeKinds.length - 1);

    expect(matrix).toHaveLength(expectedCount);
    expect(matrix.every((entry) => entry.count === expectedCount)).toBe(true);
    for (const file of files) {
      const entries = matrix.filter((entry) => entry.file === file);
      if (RUNTIME_PAIRED_FILES.has(file)) {
        expect(entries.map((entry) => entry.runtimeKind)).toEqual([...runtimeKinds]);
        for (const entry of entries) {
          expect(entry.parityCells).toHaveLength(3);
          expect(entry.parityCells.every((cell) => cell.runtime === entry.runtimeKind)).toBe(true);
          expect(new Set(entry.parityCells.map((cell) => cell.scenarioId))).toEqual(new Set([
            "public.admission-and-identity",
            "public.conversation",
            "public.files-and-checkpoints"
          ]));
        }
      } else {
        expect(entries).toHaveLength(1);
        expect(entries[0]?.runtimeKind).toBeNull();
        expect(entries[0]?.parityCells).toEqual([]);
      }
    }
  });

  it("fails closed when the authenticated runtime set is empty or duplicated", () => {
    expect(() => buildFileMatrix(["a.ts"], [])).toThrow(/non-empty/);
    expect(() => buildFileMatrix(["a.ts"], ["container", "container"])).toThrow(/unique/);
  });

  it("collects only gating live tests", () => {
    const files = collectTestFiles(userTestsRoot);

    expect(files.length).toBeGreaterThan(0);
    expect(files.every((file) => file.startsWith("test/live/"))).toBe(true);
    expect(files.every((file) => file.endsWith(".test.ts"))).toBe(true);
    expect(files.some((file) => file.startsWith("test/offline/"))).toBe(false);
    expect(files.some((file) => file.startsWith("test/_fixtures/"))).toBe(false);
    const configuredExclusions = new Set(LIVE_TEST_SHARD_CONFIG.excludedFiles);
    expect(files).toEqual(allLiveTestFiles().filter((file) => !configuredExclusions.has(file)));
    for (const file of configuredExclusions) {
      expect(existsSync(resolve(userTestsRoot, file)), file).toBe(true);
      expect(files).not.toContain(file);
    }
    expect(files.some((file) => file.includes("/providers/"))).toBe(false);
  });

  it("keeps independent chat-session scenarios in independent matrix jobs", () => {
    const files = collectTestFiles(userTestsRoot);
    const matrix = buildFileMatrix(files);
    const shards = Object.values(EDGE_CHAT_SESSION_SHARDS);
    const shardFiles = shards.map(({ file }) => file).sort();
    const shardScenarios = shards.map(({ scenario }) => scenario);

    expect(new Set(shardFiles).size).toBe(shards.length);
    expect(new Set(shardScenarios).size).toBe(shards.length);
    expect(files.filter((file) => file.startsWith("test/live/edge-chat-")).sort()).toEqual(shardFiles);
    for (const { file } of shards) {
      expect(matrix.filter((entry) => entry.file === file)).toHaveLength(1);
      expect(matrix.find((entry) => entry.file === file)?.sessionSlots).toBe(1);
    }
  });

  it("collects at least one registered test from every chat-session shard and nothing else", () => {
    // Dry collection under the REAL runner: `bun test -t <never-matching>`
    // loads every shard file, registers its describe/it tree, runs NO test and
    // NO hooks (no install, no live spend), exits nonzero with "matched 0
    // tests", and still writes a junit report listing every collected test as
    // <skipped/> under its source file. A shard file that stops registering
    // tests simply vanishes from the report and fails the set equality below —
    // the same dead-shard signal `bun x vitest list` used to give.
    const shards = Object.values(EDGE_CHAT_SESSION_SHARDS);
    const neverMatching = "AEX_SHARD_DRY_COLLECTION_NEVER_MATCHES_98f2c1";
    const reportDir = mkdtempSync(join(tmpdir(), "aex-shard-collection-"));
    const reportPath = join(reportDir, "collection-junit.xml");
    try {
      const result = spawnSync(
        "bun" in process.versions ? process.execPath : "bun",
        [
          "test",
          ...shards.map(({ file }) => file),
          "-t",
          neverMatching,
          "--reporter=junit",
          `--reporter-outfile=${reportPath}`
        ],
        {
          cwd: userTestsRoot,
          encoding: "utf8",
          // This starts a nested bun test CLI. Bound a hung process without
          // applying the outer runner's unit-test timeout to cold CLI startup.
          timeout: SHARD_COLLECTION_PROCESS_TIMEOUT_MS,
          env: {
            ...process.env,
            AEX_API_URL: "https://example.invalid",
            AEX_API_KEY: "test-api-key",
            DEEPSEEK_API_KEY: "test-provider-key",
            AEX_USER_TEST_RUNTIME_KIND: "container"
          }
        }
      );
      const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;

      // The only acceptable nonzero outcome is the dry-collection one; a load
      // error in a shard file must fail here, not masquerade as collection.
      expect(output).toMatch(/matched 0 tests/);
      expect(existsSync(reportPath), output).toBe(true);

      const testcases = listJunitTestcases(readFileSync(reportPath, "utf8")) as Array<{
        readonly file: string;
        readonly fullName: string;
        readonly status: string;
      }>;
      expect(testcases.length).toBeGreaterThanOrEqual(shards.length);
      for (const testcase of testcases) {
        expect(testcase.status, testcase.fullName).toBe("skipped");
      }
      const collectedFiles = [...new Set(testcases.map(({ file }) => file.replaceAll("\\", "/")))].sort();
      expect(collectedFiles).toEqual(shards.map(({ file }) => file).sort());
    } finally {
      rmSync(reportDir, { recursive: true, force: true });
    }
  }, SHARD_COLLECTION_PROCESS_TIMEOUT_MS + 5_000);

  it("partitions the real collected suite completely and deterministically", () => {
    const files = collectTestFiles(userTestsRoot);
    const durations = loadDurations();
    const shardCount = Math.max(1, Math.ceil(files.length / 2));

    const bins = lptPartition(files, durations, shardCount);
    const all = bins.flatMap((bin) => bin.files);
    // Completeness + disjointness: every collected file in exactly one shard.
    expect([...all].sort()).toEqual([...files].sort());
    expect(new Set(all).size).toBe(files.length);
    for (const bin of bins) expect(bin.files.length).toBeGreaterThan(0);

    // Deterministic: same inputs => identical partition.
    const again = lptPartition(files, durations, shardCount);
    expect(again.map((b) => b.files)).toEqual(bins.map((b) => b.files));
  });

  it("declares peak session-slot demand for internally concurrent live tests", () => {
    const overrideEntries = Object.entries(LIVE_TEST_SHARD_CONFIG.sessionSlotOverrides);
    expect(overrideEntries.length).toBeGreaterThan(0);
    const [overriddenFile, overriddenSlots] = overrideEntries[0]!;
    const ordinary = collectTestFiles(userTestsRoot).find((file) => !LIVE_TEST_SHARD_CONFIG.sessionSlotOverrides[file]);
    if (!ordinary) throw new Error("expected an ordinary live test file");

    expect(sessionSlotsForFile(ordinary)).toBe(1);
    expect(overriddenSlots).toBeGreaterThan(1);
    expect(sessionSlotsForFile(overriddenFile)).toBe(overriddenSlots);
    expect(declaredPeakSessionSlots([ordinary, overriddenFile])).toBe(1 + overriddenSlots);

    const files = collectTestFiles(userTestsRoot);
    const matrix = buildFileMatrix(files);
    expect(declaredPeakSessionSlots(files)).toBe(
      matrix.reduce((total, entry) => total + entry.sessionSlots, 0)
    );
    expect(declaredPeakSessionSlots(files)).toBeGreaterThan(files.length);
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
    const excludedFile = files[0];
    if (!excludedFile) throw new Error("expected a collected live test file");
    const filtered = excludeFiles(files, [excludedFile]);
    const durations = loadDurations();

    expect(filtered).not.toContain(excludedFile);
    expect(filtered.length).toBe(files.length - 1);

    const shardCount = Math.max(1, Math.ceil(filtered.length / 2));
    const bins = lptPartition(filtered, durations, shardCount);
    expect(bins).toHaveLength(shardCount);
    for (const bin of bins) expect(bin.files.length).toBeGreaterThan(0);
    expect(bins.flatMap((bin) => bin.files)).not.toContain(excludedFile);
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
