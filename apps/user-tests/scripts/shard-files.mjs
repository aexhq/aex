// CI file discovery and duration-balanced sharding for the live user-test suite.
//
// A count-based `--shard=i/N` split is a poor fit here: per-file wall
// times here span ~1s to ~6.5min (live tests wait on remote sessions), so
// count-based shards were observed at 1m42s..12m7s. This script instead
// LPT bin-packs the collected live files using recorded durations
// (shard-durations.json; unknown files get the median) and prints the file
// list for shard i of N. The hosted workflow uses `--matrix` for full
// one-file-per-job fanout; the duration-balanced modes remain useful when a
// caller intentionally chooses fewer jobs.
//
// Guarantees:
//   - deterministic: same files + same durations => same partition;
//   - every collected file lands in exactly one shard;
//   - an empty shard (or an out-of-range shard index) fails loudly, so a
//     matrix job can never silently pass with zero coverage.
//
// Usage:
//   node scripts/shard-files.mjs --matrix --runtime-capabilities-json <json> [--exclude-file <rel>]...
//   node scripts/shard-files.mjs --shard <i>/<N> [--exclude-file <rel>]...
//   node scripts/shard-files.mjs --summary <N> [--exclude-file <rel>]...
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseRuntimeCapabilities } from "../../../scripts/cicd/runtime-capabilities.mjs";
import { parityCellsForFile } from "./runtime-parity-verdicts.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(here, "..");

// Implementation-owned live-test policy. The collector, matrix builder, and
// validators all consume this manifest so exclusions and resource declarations
// do not drift into separate test inventories.
export const LIVE_TEST_SHARD_CONFIG = Object.freeze({
  // Cap-saturating suites run in dedicated workflow lanes with isolated
  // workspaces so they cannot starve unrelated live assertions.
  excludedFiles: Object.freeze([
    "test/live/edge-admission-gates.user.test.ts",
    "test/live/live-sdk-heavy-session.test.ts",
    "test/live/live-api-fuzz.test.ts",
    "test/live/live-sdk-tool-capability-fuzz.test.ts"
  ]),
  excludedDirectories: Object.freeze(["node_modules", "providers"]),
  // Most live files run at most one active session at a time. Values here are
  // the declared peak for files that start sessions concurrently in a test.
  sessionSlotOverrides: Object.freeze({
    "test/live/edge-concurrency-scale.user.test.ts": 10
  }),
  // Only these files explicitly submit the selected runtime and assert the
  // returned session identity. Other files must remain container-only.
  runtimePairedFiles: Object.freeze([
    "test/live/edge-cli.user.test.ts",
    "test/live/live-sdk-event-stream.test.ts"
  ])
});

const EXCLUDED = new Set(LIVE_TEST_SHARD_CONFIG.excludedFiles);
const EXCLUDED_DIRS = new Set(LIVE_TEST_SHARD_CONFIG.excludedDirectories);
const SESSION_SLOT_OVERRIDES = new Map(Object.entries(LIVE_TEST_SHARD_CONFIG.sessionSlotOverrides));
export const RUNTIME_PAIRED_FILES = new Set(LIVE_TEST_SHARD_CONFIG.runtimePairedFiles);

export function collectTestFiles(root = appRoot) {
  const out = [];
  const walk = (rel) => {
    for (const entry of readdirSync(join(root, rel), { withFileTypes: true })) {
      const relPath = `${rel}/${entry.name}`;
      if (entry.isDirectory()) {
        if (!EXCLUDED_DIRS.has(entry.name)) walk(relPath);
      } else if (entry.name.endsWith(".test.ts") && !EXCLUDED.has(relPath)) {
        out.push(relPath);
      }
    }
  };
  walk("test/live");
  return out.sort();
}

export function sessionSlotsForFile(file) {
  return SESSION_SLOT_OVERRIDES.get(file) ?? 1;
}

export function declaredPeakSessionSlots(files) {
  return files.reduce((total, file) => total + sessionSlotsForFile(file), 0);
}

export function loadDurations(path = join(appRoot, "shard-durations.json")) {
  const raw = JSON.parse(readFileSync(path, "utf8"));
  const durations = new Map();
  for (const [key, value] of Object.entries(raw)) {
    if (key.startsWith("$")) continue;
    if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
      throw new Error(`shard-durations.json: invalid duration for ${key}: ${value}`);
    }
    durations.set(key, value);
  }
  if (durations.size === 0) throw new Error("shard-durations.json has no durations");
  return durations;
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

// Deterministic LPT (longest-processing-time-first) bin packing.
// Returns N bins of { files, seconds }; every input file lands in exactly
// one bin, and no bin may be empty.
export function lptPartition(files, durations, shardCount) {
  if (!Number.isInteger(shardCount) || shardCount < 1) {
    throw new Error(`invalid shard count: ${shardCount}`);
  }
  if (files.length === 0) throw new Error("no test files collected");
  if (files.length < shardCount) {
    throw new Error(
      `only ${files.length} test files for ${shardCount} shards — a shard would be empty`
    );
  }
  const defaultSeconds = median([...durations.values()]);
  const weighted = files
    .map((file) => ({ file, seconds: durations.get(file) ?? defaultSeconds }))
    // Duration desc, then path asc: fully deterministic order.
    .sort((a, b) => b.seconds - a.seconds || (a.file < b.file ? -1 : 1));

  const bins = Array.from({ length: shardCount }, () => ({ files: [], seconds: 0 }));
  for (const { file, seconds } of weighted) {
    // Least-loaded bin; ties go to the lowest index (deterministic).
    let target = 0;
    for (let i = 1; i < shardCount; i++) {
      if (bins[i].seconds < bins[target].seconds) target = i;
    }
    bins[target].files.push(file);
    bins[target].seconds += seconds;
  }
  for (const [i, bin] of bins.entries()) {
    if (bin.files.length === 0) throw new Error(`shard ${i + 1}/${shardCount} is empty`);
    bin.files.sort();
  }
  return bins;
}

export function excludeFiles(files, excludedFiles) {
  if (excludedFiles.length === 0) return files;
  const available = new Set(files);
  const missing = excludedFiles.filter((file) => !available.has(file));
  if (missing.length > 0) {
    throw new Error(`excluded file(s) are not collected: ${missing.join(", ")}`);
  }
  const excluded = new Set(excludedFiles);
  return files.filter((file) => !excluded.has(file));
}

export function buildFileMatrix(files, runtimeKinds = ["container"]) {
  if (files.length === 0) throw new Error("no test files collected");
  if (!Array.isArray(runtimeKinds) || runtimeKinds.length === 0) {
    throw new Error("runtime kinds must be a non-empty array");
  }
  if (new Set(runtimeKinds).size !== runtimeKinds.length) {
    throw new Error("runtime kinds must be unique");
  }
  const entries = files.flatMap((file) =>
    RUNTIME_PAIRED_FILES.has(file)
      ? runtimeKinds.map((runtimeKind) => ({ file, runtimeKind }))
      : [{ file, runtimeKind: null }]
  );
  const count = entries.length;
  return entries.map(({ file, runtimeKind }, index) => ({
    shard: index + 1,
    count,
    file,
    runtimeKind,
    parityCells: parityCellsForFile(file, runtimeKind),
    sessionSlots: sessionSlotsForFile(file)
  }));
}

function parseArgs(argv) {
  let mode;
  let value;
  const excludedFiles = [];
  let runtimeCapabilitiesJson;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--matrix") {
      if (mode !== undefined) throw new Error("choose only one of --matrix, --shard, or --summary");
      mode = arg;
    } else if (arg === "--shard" || arg === "--summary") {
      if (mode !== undefined) throw new Error("choose only one of --shard or --summary");
      mode = arg;
      value = argv[++i];
      if (value === undefined) throw new Error(`${arg} requires a value`);
    } else if (arg === "--exclude-file") {
      const file = argv[++i];
      if (file === undefined || file === "") throw new Error("--exclude-file requires a relative test path");
      excludedFiles.push(file);
    } else if (arg === "--runtime-capabilities-json") {
      runtimeCapabilitiesJson = argv[++i];
      if (runtimeCapabilitiesJson === undefined || runtimeCapabilitiesJson === "") {
        throw new Error("--runtime-capabilities-json requires a JSON value");
      }
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return { mode, value, excludedFiles, runtimeCapabilitiesJson };
}

function main(argv) {
  const { mode, value, excludedFiles, runtimeCapabilitiesJson } = parseArgs(argv);
  const files = excludeFiles(collectTestFiles(), excludedFiles);
  if (mode === "--matrix") {
    if (!runtimeCapabilitiesJson) {
      throw new Error("--matrix requires authenticated --runtime-capabilities-json");
    }
    let rawCapabilities;
    try {
      rawCapabilities = JSON.parse(runtimeCapabilitiesJson);
    } catch {
      throw new Error("--runtime-capabilities-json must be valid JSON");
    }
    const capabilities = parseRuntimeCapabilities(rawCapabilities);
    process.stdout.write(`${JSON.stringify(buildFileMatrix(files, capabilities.availableRuntimeKinds))}\n`);
    return;
  }
  const durations = loadDurations();
  if (mode === "--shard") {
    const match = /^([0-9]+)\/([0-9]+)$/.exec(value ?? "");
    if (!match) throw new Error(`--shard expects <i>/<N>, got: ${value}`);
    const index = Number(match[1]);
    const count = Number(match[2]);
    const bins = lptPartition(files, durations, count);
    if (index < 1 || index > count) throw new Error(`shard index ${index} out of range 1..${count}`);
    process.stdout.write(bins[index - 1].files.join("\n") + "\n");
    return;
  }
  if (mode === "--summary") {
    const count = Number(value);
    const bins = lptPartition(files, durations, count);
    for (const [i, bin] of bins.entries()) {
      console.log(`shard ${i + 1}/${count}: ${Math.round(bin.seconds)}s (${bin.files.length} files)`);
    }
    return;
  }
  throw new Error("usage: shard-files.mjs --matrix --runtime-capabilities-json <json> [--exclude-file <rel>]... | --shard <i>/<N> [--exclude-file <rel>]... | --summary <N> [--exclude-file <rel>]...");
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`shard-files: ${error instanceof Error ? error.message : error}`);
    process.exit(1);
  }
}
