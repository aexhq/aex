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
  collectAllLiveFiles,
  collectTestFiles,
  coverageFor,
  declaredPeakSessionSlots,
  excludeFiles,
  filesInTier,
  loadDurations,
  lptPartition,
  LIVE_TEST_COVERAGE,
  LIVE_TEST_SHARD_CONFIG,
  ON_DEMAND_FILES,
  sessionSlotsForFile
} from "../../apps/user-tests/scripts/shard-files.mjs";
import type { ShardBin } from "../../apps/user-tests/scripts/shard-files.mjs";

/** The dev plane's declared arms, per the platform scenario ledger. */
const DEV_COVERAGE = {
  fullCoverage: ["lambda", "spot_container"]
} as const;

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
  it("covers every collected file exactly once, with contiguous shard numbering", () => {
    const files = collectTestFiles(userTestsRoot);
    const matrix = buildFileMatrix(files, DEV_COVERAGE);
    const covered = matrix.flatMap((entry) => entry.files);

    expect([...new Set(covered)].sort()).toEqual([...files].sort());
    expect(matrix.map((entry) => entry.shard)).toEqual(matrix.map((_, index) => index + 1));
    expect(matrix.every((entry) => entry.count === matrix.length)).toBe(true);
    // `file` is the argv string CI forwards; `files` is the structured list.
    expect(matrix.every((entry) => entry.file === entry.files.join(" "))).toBe(true);
    expect(
      matrix.every(
        (entry) => entry.sessionSlots === Math.max(...entry.files.map((file) => sessionSlotsForFile(file)))
      )
    ).toBe(true);
  });

  it("fans each file over exactly the arms its declared tier owes", () => {
    const files = collectTestFiles(userTestsRoot);
    const matrix = buildFileMatrix(files, DEV_COVERAGE);
    const armsFor = (file: string): string[] =>
      matrix
        .filter((entry) => entry.files.includes(file))
        .map((entry) => String(entry.runtimeKind))
        .sort();

    for (const file of filesInTier("runtime-spotcheck", files)) {
      expect(armsFor(file), file).toEqual(["lambda", "spot_container"]);
      for (const entry of matrix.filter((candidate) => candidate.files.includes(file))) {
        expect(entry.parityCells).toHaveLength(3);
        expect(entry.parityCells.every((cell) => cell.runtime === entry.runtimeKind)).toBe(true);
        expect(new Set(entry.parityCells.map((cell) => cell.scenarioId))).toEqual(new Set([
          "public.admission-and-identity",
          "public.conversation",
          "public.files-and-checkpoints"
        ]));
      }
    }
    for (const file of filesInTier("runtime-matrix", files)) {
      expect(armsFor(file), file).toEqual(["lambda", "spot_container"]);
    }
    for (const file of filesInTier("runtime-agnostic", files)) {
      expect(armsFor(file), file).toEqual(["spot_container"]);
    }
    // Only spotcheck files carry parity cells; everything else emits none, and
    // the verdict emitter fails closed on an empty cell list.
    const spotcheck = new Set(filesInTier("runtime-spotcheck", files));
    for (const entry of matrix) {
      if (entry.files.some((file) => spotcheck.has(file))) continue;
      expect(entry.parityCells, entry.file).toEqual([]);
    }
  });

  it("pins agnostic bins to the shipped container default regardless of capability order", () => {
    const files = collectTestFiles(userTestsRoot);
    const publicCapabilityOrder = ["container", "spot_container", "lambda"] as const;
    const matrix = buildFileMatrix(files, { fullCoverage: publicCapabilityOrder });
    for (const entry of matrix.filter((candidate) => candidate.tier === "runtime-agnostic")) {
      expect(entry.runtimeKind, entry.file).toBe("spot_container");
    }
  });

  it("stays materially smaller than one job per file per arm", () => {
    const files = collectTestFiles(userTestsRoot);
    const naive = files.length * DEV_COVERAGE.fullCoverage.length;
    expect(buildFileMatrix(files, DEV_COVERAGE).length).toBeLessThan(naive / 2);
  });

  it("fails closed on an empty or duplicated runtime set", () => {
    const files = collectTestFiles(userTestsRoot);
    expect(() => buildFileMatrix(files, { fullCoverage: [] })).toThrow(/non-empty/);
    expect(() => buildFileMatrix(files, { fullCoverage: ["container", "container"] })).toThrow(/unique/);
  });

  it("requires every live file to declare a coverage tier, and every declaration to resolve", () => {
    const onDisk = collectAllLiveFiles(userTestsRoot);
    expect(Object.keys(LIVE_TEST_COVERAGE).sort()).toEqual([...onDisk].sort());
    for (const file of onDisk) {
      const entry = coverageFor(file);
      expect(["sdk", "cli"], file).toContain(entry.entryPoint);
      expect(entry.reason.trim().length, file).toBeGreaterThanOrEqual(40);
    }
    // The on-demand tier and the sweep exclusion list are the same set, derived
    // from one declaration rather than kept in step by hand.
    expect([...LIVE_TEST_SHARD_CONFIG.excludedFiles].sort()).toEqual([...ON_DEMAND_FILES].sort());
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

  it("keeps every chat-session scenario collected and uniquely registered", () => {
    const files = collectTestFiles(userTestsRoot);
    const matrix = buildFileMatrix(files, DEV_COVERAGE);
    const shards = Object.values(EDGE_CHAT_SESSION_SHARDS);
    const shardFiles = shards.map(({ file }) => file).sort();
    const shardScenarios = shards.map(({ scenario }) => scenario);

    expect(new Set(shardFiles).size).toBe(shards.length);
    expect(new Set(shardScenarios).size).toBe(shards.length);
    expect(files.filter((file) => file.startsWith("test/live/edge-chat-")).sort()).toEqual(shardFiles);
    for (const { file } of shards) {
      const entries = matrix.filter((entry) => entry.files.includes(file));
      expect(entries.length, file).toBeGreaterThan(0);
      // A shard is never duplicated on one arm, whatever tier it sits in.
      expect(new Set(entries.map((entry) => entry.runtimeKind)).size, file).toBe(entries.length);
      expect(entries.every((entry) => entry.sessionSlots === 1), file).toBe(true);
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

    // A matrix entry's declared demand is its peak, not its sum: files inside a
    // duration-packed bin run serially, so a bin never holds more concurrent
    // sessions than its greediest member.
    const files = collectTestFiles(userTestsRoot);
    const matrix = buildFileMatrix(files, DEV_COVERAGE);
    for (const entry of matrix) {
      expect(entry.sessionSlots, entry.file).toBe(
        Math.max(...entry.files.map((file) => sessionSlotsForFile(file)))
      );
    }
    const peak = matrix.reduce((total, entry) => total + entry.sessionSlots, 0);
    expect(peak).toBe(matrix.length);
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
