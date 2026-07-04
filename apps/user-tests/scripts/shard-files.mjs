// Duration-balanced CI test sharding for the live user-test suite.
//
// Vitest's built-in `--shard=i/N` splits by FILE COUNT, but per-file wall
// times here span ~1s to ~6.5min (live tests wait on remote runs), so
// count-based shards were observed at 1m42s..12m7s. This script instead
// LPT bin-packs the files vitest would collect using recorded durations
// (shard-durations.json; unknown files get the median) and prints the file
// list for shard i of N. Workflows run `test:user:files -- <files...>`
// instead of `--shard`.
//
// Guarantees:
//   - deterministic: same files + same durations => same partition;
//   - every collected file lands in exactly one shard;
//   - an empty shard (or an out-of-range shard index) fails loudly, so a
//     matrix job can never silently pass with zero coverage.
//
// Usage:
//   node scripts/shard-files.mjs --shard <i>/<N>      # newline-separated files for shard i
//   node scripts/shard-files.mjs --summary <N>        # per-shard predicted seconds (all shards)
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(here, "..");

// MUST mirror the `exclude` list in vitest.config.ts (the default `test:user`
// sweep). Heavy/fuzz/provider suites are separate explicit gates.
const EXCLUDED = new Set([
  // Cap-saturating by design. It runs in a dedicated workflow lane with an
  // isolated low-cap workspace so it cannot starve unrelated live assertions.
  "test/live/edge-admission-gates.user.test.ts",
  "test/live/live-sdk-heavy-session.test.ts",
  "test/live/live-api-fuzz.test.ts",
  "test/live/live-sdk-tool-capability-fuzz.test.ts"
]);
const EXCLUDED_DIRS = new Set(["node_modules", "providers"]);

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
  walk("test");
  return out.sort();
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

function main(argv) {
  const [flag, value] = argv;
  const durations = loadDurations();
  const files = collectTestFiles();
  if (flag === "--shard") {
    const match = /^([0-9]+)\/([0-9]+)$/.exec(value ?? "");
    if (!match) throw new Error(`--shard expects <i>/<N>, got: ${value}`);
    const index = Number(match[1]);
    const count = Number(match[2]);
    const bins = lptPartition(files, durations, count);
    if (index < 1 || index > count) throw new Error(`shard index ${index} out of range 1..${count}`);
    process.stdout.write(bins[index - 1].files.join("\n") + "\n");
    return;
  }
  if (flag === "--summary") {
    const count = Number(value);
    const bins = lptPartition(files, durations, count);
    for (const [i, bin] of bins.entries()) {
      console.log(`shard ${i + 1}/${count}: ${Math.round(bin.seconds)}s (${bin.files.length} files)`);
    }
    return;
  }
  throw new Error("usage: shard-files.mjs --shard <i>/<N> | --summary <N>");
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`shard-files: ${error instanceof Error ? error.message : error}`);
    process.exit(1);
  }
}
