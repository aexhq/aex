// Deterministic duration-balanced sharding for the live v1 user-test suite.
//
// The v1 architecture has one execution path. This collector therefore shards
// files, not runtime kinds or capability cells.
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(here, "..");

export const LIVE_TEST_SHARD_CONFIG = Object.freeze({
  excludedDirectories: Object.freeze(["node_modules"]),
  defaultShards: 4
});

export function collectTestFiles(root = appRoot) {
  const out = [];
  const walk = (relative) => {
    for (const entry of readdirSync(join(root, relative), { withFileTypes: true })) {
      const path = `${relative}/${entry.name}`;
      if (entry.isDirectory()) {
        if (!LIVE_TEST_SHARD_CONFIG.excludedDirectories.includes(entry.name)) walk(path);
      } else if (entry.isFile() && entry.name.endsWith(".test.ts")) {
        out.push(path);
      }
    }
  };
  walk("test/live");
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
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1
    ? sorted[middle]
    : (sorted[middle - 1] + sorted[middle]) / 2;
}

export function lptPartition(files, durations, shardCount) {
  if (!Number.isInteger(shardCount) || shardCount < 1) {
    throw new Error(`invalid shard count: ${shardCount}`);
  }
  if (files.length === 0) throw new Error("no test files collected");
  if (files.length < shardCount) {
    throw new Error(`only ${files.length} test files for ${shardCount} shards — a shard would be empty`);
  }
  const fallback = median([...durations.values()]);
  const weighted = files
    .map((file) => ({ file, seconds: durations.get(file) ?? fallback }))
    .sort((left, right) => right.seconds - left.seconds || left.file.localeCompare(right.file));
  const bins = Array.from({ length: shardCount }, () => ({ files: [], seconds: 0 }));
  for (const item of weighted) {
    let target = 0;
    for (let index = 1; index < bins.length; index += 1) {
      if (bins[index].seconds < bins[target].seconds) target = index;
    }
    bins[target].files.push(item.file);
    bins[target].seconds += item.seconds;
  }
  for (const bin of bins) bin.files.sort();
  return bins;
}

export function excludeFiles(files, excludedFiles) {
  const available = new Set(files);
  const missing = excludedFiles.filter((file) => !available.has(file));
  if (missing.length > 0) throw new Error(`excluded file(s) are not collected: ${missing.join(", ")}`);
  const excluded = new Set(excludedFiles);
  return files.filter((file) => !excluded.has(file));
}

export function buildFileMatrix(files, options = {}) {
  if (files.length === 0) throw new Error("no test files collected");
  const requested = options.shards ?? LIVE_TEST_SHARD_CONFIG.defaultShards;
  const shardCount = Math.min(requested, files.length);
  const bins = lptPartition(files, options.durations ?? loadDurations(), shardCount);
  return bins.map((bin, index) => ({
    shard: index + 1,
    count: bins.length,
    file: bin.files.join(" "),
    files: bin.files,
    sessionSlots: 1
  }));
}

function positiveInteger(raw, flag) {
  const value = Number(raw);
  if (!Number.isInteger(value) || value < 1) throw new Error(`${flag} requires a positive integer`);
  return value;
}

function main(argv) {
  let mode;
  let value;
  let shards = LIVE_TEST_SHARD_CONFIG.defaultShards;
  const excludedFiles = [];
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--matrix") {
      mode = "matrix";
    } else if (arg === "--shard" || arg === "--summary") {
      mode = arg.slice(2);
      value = argv[++index];
    } else if (arg === "--shards") {
      shards = positiveInteger(argv[++index], arg);
    } else if (arg === "--exclude-file") {
      const file = argv[++index];
      if (!file) throw new Error("--exclude-file requires a relative test path");
      excludedFiles.push(file);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  const files = excludeFiles(collectTestFiles(), excludedFiles);
  if (mode === "matrix") {
    process.stdout.write(`${JSON.stringify(buildFileMatrix(files, { shards }))}\n`);
    return;
  }
  if (mode === "shard") {
    const match = /^([0-9]+)\/([0-9]+)$/.exec(value ?? "");
    if (!match) throw new Error(`--shard expects <i>/<N>, got: ${value}`);
    const index = Number(match[1]);
    const count = Number(match[2]);
    const bins = lptPartition(files, loadDurations(), count);
    if (index < 1 || index > count) throw new Error(`shard index ${index} out of range 1..${count}`);
    process.stdout.write(`${bins[index - 1].files.join("\n")}\n`);
    return;
  }
  if (mode === "summary") {
    const bins = lptPartition(files, loadDurations(), positiveInteger(value, "--summary"));
    bins.forEach((bin, index) => {
      console.log(`shard ${index + 1}/${bins.length}: ${Math.round(bin.seconds)}s (${bin.files.length} files)`);
    });
    return;
  }
  throw new Error("usage: shard-files.mjs --matrix [--shards N] | --shard i/N | --summary N");
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`shard-files: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
}
