// CI file discovery, release-matrix construction, and duration-balanced sharding
// for the live user-test suite.
//
// The per-file coverage TIERS (which files fan out over which runtime arms, and
// why) live in ./live-coverage.mjs and are re-exported here so callers have one
// import. The runtime KINDS live in the platform scenario ledger and are passed
// in by the caller. This module owns only the mechanics: walk the tree, assert
// the manifest, bin-pack, and emit matrix entries.
//
// SHARDING
// --------
// A count-based split is a poor fit: per-file wall times span ~1s to ~6.5min
// (live tests wait on remote sessions), so count-based shards were observed at
// 1m42s..12m7s. The runtime-agnostic tier is LPT bin-packed using recorded
// durations (shard-durations.json; unknown files get the median).
//
// Guarantees:
//   - deterministic: same files + same durations => same partition;
//   - every collected file lands in exactly one matrix entry;
//   - an empty shard (or an out-of-range shard index) fails loudly, so a
//     matrix job can never silently pass with zero coverage.
//
// Usage:
//   node scripts/shard-files.mjs --matrix --full-coverage-kinds <json> [--agnostic-shards <n>]
//   node scripts/shard-files.mjs --matrix --runtime-capabilities-json <json> [--agnostic-shards <n>]
//   node scripts/shard-files.mjs --shard <i>/<N> [--exclude-file <rel>]...
//   node scripts/shard-files.mjs --summary <N> [--exclude-file <rel>]...
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseRuntimeCapabilities } from "../../../scripts/cicd/runtime-capabilities.mjs";
import {
  COVERAGE_TIERS,
  LIVE_TEST_COVERAGE,
  ON_DEMAND_FILES,
  assertCoverageManifest,
  coverageFor
} from "./live-coverage.mjs";
import { parityCellsForFile } from "./runtime-parity-verdicts.mjs";

export { COVERAGE_TIERS, LIVE_TEST_COVERAGE, ON_DEMAND_FILES, assertCoverageManifest, coverageFor };

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(here, "..");

// Implementation-owned live-test policy. The collector, matrix builder, and
// validators all consume this manifest so exclusions and resource declarations
// do not drift into separate test inventories.
export const LIVE_TEST_SHARD_CONFIG = Object.freeze({
  // Cap-saturating suites run in dedicated workflow lanes with isolated
  // workspaces so they cannot starve unrelated live assertions. Derived from the
  // coverage manifest so the tier and the exclusion cannot disagree.
  excludedFiles: ON_DEMAND_FILES,
  excludedDirectories: Object.freeze(["node_modules"]),
  // Most live files run at most one active session at a time. Values here are
  // the declared peak for files that start sessions concurrently in a test.
  sessionSlotOverrides: Object.freeze({
    "test/live/edge-concurrency-scale.user.test.ts": 10
  }),
  // How many duration-balanced jobs the runtime-agnostic tier is packed into.
  // Four keeps each shard near the longest single matrix file, so the agnostic
  // tier stops being the critical path without hiding many files behind one red.
  defaultAgnosticShards: 4
});

const EXCLUDED = new Set(LIVE_TEST_SHARD_CONFIG.excludedFiles);
const EXCLUDED_DIRS = new Set(LIVE_TEST_SHARD_CONFIG.excludedDirectories);
const SESSION_SLOT_OVERRIDES = new Map(Object.entries(LIVE_TEST_SHARD_CONFIG.sessionSlotOverrides));

/** Every live test file on disk, including the on-demand lanes. */
export function collectAllLiveFiles(root = appRoot) {
  const out = [];
  const walk = (rel) => {
    for (const entry of readdirSync(join(root, rel), { withFileTypes: true })) {
      const relPath = `${rel}/${entry.name}`;
      if (entry.isDirectory()) {
        if (!EXCLUDED_DIRS.has(entry.name)) walk(relPath);
      } else if (entry.name.endsWith(".test.ts")) {
        out.push(relPath);
      }
    }
  };
  walk("test/live");
  return out.sort();
}

/**
 * The live files the release gate sweeps: everything on disk minus the
 * on-demand tier. Classification is asserted here rather than at the matrix
 * builder, so a new unclassified file fails the sweep too.
 */
export function collectTestFiles(root = appRoot) {
  return assertCoverageManifest(collectAllLiveFiles(root)).filter((file) => !EXCLUDED.has(file));
}

/** The sweep files in one coverage tier, sorted. */
export function filesInTier(tier, files = collectTestFiles()) {
  if (!COVERAGE_TIERS.includes(tier)) throw new Error(`unknown coverage tier: ${tier}`);
  return files.filter((file) => coverageFor(file).tier === tier);
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

function assertKindList(kinds, label, { allowEmpty = false } = {}) {
  if (!Array.isArray(kinds)) throw new Error(`${label} must be an array`);
  if (!allowEmpty && kinds.length === 0) throw new Error(`${label} must be a non-empty array`);
  if (new Set(kinds).size !== kinds.length) throw new Error(`${label} must be unique`);
  return kinds;
}

/**
 * The release matrix for one plane.
 *
 * Each entry is one CI job. `files` is a LIST because the runtime-agnostic tier
 * is duration-packed: a job runs one file on a runtime arm, or a bin of files on
 * the plane's primary arm.
 *
 * @param files            the swept live files (on-demand already removed)
 * @param coverage.fullCoverage    runtime kinds owing full ledger coverage
 * @param coverage.agnosticShards  bins for the runtime-agnostic tier
 */
export function buildFileMatrix(files, coverage = {}) {
  if (files.length === 0) throw new Error("no test files collected");
  const fullCoverage = assertKindList(coverage.fullCoverage ?? ["spot_container"], "full-coverage runtime kinds");
  const agnosticShards = coverage.agnosticShards ?? LIVE_TEST_SHARD_CONFIG.defaultAgnosticShards;

  // The plane's PRIMARY arm carries the runtime-agnostic tier. Prefer the
  // shipped default explicitly: authenticated runtime capabilities use public
  // contract order (container, spot_container, lambda), while the deploy
  // ledger uses failure-cost order (lambda first, containers after). "Last"
  // therefore silently selected Lambda in the standalone live workflow.
  const primaryRuntimeKind = fullCoverage.includes("spot_container")
    ? "spot_container"
    : fullCoverage.includes("container")
      ? "container"
      : fullCoverage[fullCoverage.length - 1];

  const entries = [];
  for (const file of files) {
    const { tier } = coverageFor(file);
    if (tier === "on-demand") throw new Error(`on-demand file reached the release matrix: ${file}`);
    if (tier === "runtime-agnostic") continue;
    for (const runtimeKind of fullCoverage) entries.push({ files: [file], runtimeKind });
  }

  const agnostic = filesInTier("runtime-agnostic", files);
  if (agnostic.length > 0) {
    // Fewer files than bins would fail loudly in lptPartition; clamp instead so
    // deleting agnostic files never breaks the build, only shrinks the fan-out.
    const bins = lptPartition(agnostic, loadDurations(), Math.min(agnosticShards, agnostic.length));
    for (const bin of bins) entries.push({ files: bin.files, runtimeKind: primaryRuntimeKind });
  }

  const count = entries.length;
  return entries.map(({ files: entryFiles, runtimeKind }, index) => ({
    shard: index + 1,
    count,
    // `file` stays a single space-joined argv string so the workflow's
    // `test:user:files -- $AEX_USER_TEST_FILE` invocation is unchanged.
    file: entryFiles.join(" "),
    files: entryFiles,
    runtimeKind,
    tier: coverageFor(entryFiles[0]).tier,
    parityCells: entryFiles.flatMap((entryFile) => parityCellsForFile(entryFile, runtimeKind)),
    sessionSlots: Math.max(...entryFiles.map((entryFile) => sessionSlotsForFile(entryFile)))
  }));
}

function parseJsonArg(raw, flag) {
  try {
    return JSON.parse(raw);
  } catch {
    throw new Error(`${flag} must be valid JSON`);
  }
}

function parseArgs(argv) {
  let mode;
  let value;
  const excludedFiles = [];
  let runtimeCapabilitiesJson;
  let fullCoverageKindsJson;
  let agnosticShards;
  const requireValue = (flag, i) => {
    const next = argv[i];
    if (next === undefined || next === "") throw new Error(`${flag} requires a value`);
    return next;
  };
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
      runtimeCapabilitiesJson = requireValue(arg, ++i);
    } else if (arg === "--full-coverage-kinds") {
      fullCoverageKindsJson = requireValue(arg, ++i);
    } else if (arg === "--agnostic-shards") {
      agnosticShards = Number(requireValue(arg, ++i));
      if (!Number.isInteger(agnosticShards) || agnosticShards < 1) {
        throw new Error("--agnostic-shards requires a positive integer");
      }
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }
  return {
    mode,
    value,
    excludedFiles,
    runtimeCapabilitiesJson,
    fullCoverageKindsJson,
    agnosticShards
  };
}

function main(argv) {
  const {
    mode,
    value,
    excludedFiles,
    runtimeCapabilitiesJson,
    fullCoverageKindsJson,
    agnosticShards
  } = parseArgs(argv);
  const files = excludeFiles(collectTestFiles(), excludedFiles);
  if (mode === "--matrix") {
    if (runtimeCapabilitiesJson && fullCoverageKindsJson) {
      throw new Error("choose only one of --runtime-capabilities-json or --full-coverage-kinds");
    }
    let fullCoverage;
    if (fullCoverageKindsJson) {
      // The deploy gate resolves both lists from the platform scenario ledger
      // and passes them in; this script never re-derives them.
      fullCoverage = parseJsonArg(fullCoverageKindsJson, "--full-coverage-kinds");
    } else if (runtimeCapabilitiesJson) {
      // The aex-owned manual lane has no ledger, only what the authenticated
      // workspace reports. Every available kind is treated as full coverage and
      // the container spot-check stays a deploy-gate concept, so this lane can
      // never disagree with the ledger — it simply does not model the subset.
      const capabilities = parseRuntimeCapabilities(
        parseJsonArg(runtimeCapabilitiesJson, "--runtime-capabilities-json")
      );
      fullCoverage = capabilities.availableRuntimeKinds;
    } else {
      throw new Error("--matrix requires --full-coverage-kinds or authenticated --runtime-capabilities-json");
    }
    const matrix = buildFileMatrix(files, { fullCoverage, agnosticShards });
    process.stdout.write(`${JSON.stringify(matrix)}\n`);
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
  throw new Error(
    "usage: shard-files.mjs --matrix (--full-coverage-kinds <json> | --runtime-capabilities-json <json>) [--agnostic-shards <n>] [--exclude-file <rel>]... | --shard <i>/<N> [--exclude-file <rel>]... | --summary <N> [--exclude-file <rel>]..."
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`shard-files: ${error instanceof Error ? error.message : error}`);
    process.exit(1);
  }
}
