/**
 * Pure lane-argv processing for `user-bun-test.mjs`: turn a lane's argv into
 * the `bun test` argv, the junit report path, and the no-skips gate wiring.
 *
 * Split out because it is the one part of the runner with NO side effects — no
 * spawn, no filesystem, no process state — and therefore the part
 * `scripts/validate/user-bun-test-spawn.test.ts` exercises directly. Both
 * functions are re-exported from `user-bun-test.mjs`, which is still where
 * callers import them from.
 *
 * `envLike` is a parameter rather than a module import for the same reason: the
 * lane is resolved before the runner mutates its own environment copy, so the
 * caller states which environment it means.
 */
// `bun test --parallel` before this floor could exit 0 after a worker crash.
const PARALLEL_SAFE_BUN_FLOOR = "1.3.14";

// Forwarded `bun test` flags that take their value as the NEXT argv element.
// Needed to tell flag values apart from file selections; `=`-joined forms are
// self-contained and need no entry here.
const VALUE_TAKING_BUN_TEST_FLAGS = new Set([
  "-t",
  "--test-name-pattern",
  "--timeout",
  "--reporter",
  "--reporter-outfile"
]);

/**
 * Pure lane-argv processing: split wrapper-owned flags from the `bun test`
 * argv, resolve --parallel-env, and derive the junit + no-skips gate wiring.
 * Throws on any ambiguous or selection-free invocation.
 */
export function prepareLaneInvocation(argv, envLike = process.env) {
  let reportPath = null;
  let parallelSpec = null;
  let sweep = false;
  let smoke = false;
  const forwarded = [];

  for (const arg of argv) {
    if (arg === "--sweep") {
      sweep = true;
    } else if (arg === "--smoke") {
      smoke = true;
    } else if (arg === "--report" || arg.startsWith("--report=")) {
      const value = arg.startsWith("--report=") ? arg.slice("--report=".length) : "";
      if (!value) throw new Error("user-bun-test: --report requires =<junit-report-path>");
      if (reportPath !== null) throw new Error("user-bun-test: duplicate --report");
      reportPath = value;
    } else if (arg === "--parallel-env" || arg.startsWith("--parallel-env=")) {
      const value = arg.startsWith("--parallel-env=") ? arg.slice("--parallel-env=".length) : "";
      const match = /^([A-Z][A-Z0-9_]*):([0-9]+)$/.exec(value);
      if (!match) throw new Error("user-bun-test: --parallel-env requires =ENV_NAME:FALLBACK");
      if (parallelSpec !== null) throw new Error("user-bun-test: duplicate --parallel-env");
      parallelSpec = { name: match[1], fallback: Number(match[2]) };
      if (!Number.isInteger(parallelSpec.fallback) || parallelSpec.fallback < 1) {
        throw new Error("user-bun-test: --parallel-env fallback must be a positive integer");
      }
    } else {
      forwarded.push(arg);
    }
  }

  if (sweep && smoke) throw new Error("user-bun-test: choose only one of --sweep or --smoke");
  const suite = sweep ? "sweep" : smoke ? "smoke" : null;

  // Classify forwarded args: find file selections and any bun -t filter.
  let namePattern = null;
  let positionals = 0;
  for (let i = 0; i < forwarded.length; i += 1) {
    const arg = forwarded[i];
    if (arg === "-t" || arg === "--test-name-pattern") {
      namePattern = forwarded[i + 1] ?? null;
      i += 1;
      continue;
    }
    if (arg.startsWith("-t=")) {
      namePattern = arg.slice("-t=".length);
      continue;
    }
    if (arg.startsWith("--test-name-pattern=")) {
      namePattern = arg.slice("--test-name-pattern=".length);
      continue;
    }
    if (arg.startsWith("-")) {
      if (VALUE_TAKING_BUN_TEST_FLAGS.has(arg)) i += 1;
      continue;
    }
    positionals += 1;
  }

  if (suite && positionals > 0) {
    throw new Error("user-bun-test: pass either a suite selector or explicit files, not both");
  }
  if (!suite && positionals === 0) {
    throw new Error(
      "user-bun-test: refusing to spawn `bun test` without an explicit file selection " +
        "(a bare `bun test` here would collect every live suite); pass test files/directories or --sweep/--smoke"
    );
  }

  if (reportPath !== null && forwarded.some((arg) => arg === "--reporter" || arg.startsWith("--reporter="))) {
    throw new Error("user-bun-test: --report owns the junit reporter flags; do not also pass --reporter");
  }

  const bunTestArgs = [...forwarded];
  if (parallelSpec) {
    bunTestArgs.unshift(`--parallel=${resolveWorkerCount(parallelSpec.name, parallelSpec.fallback, envLike)}`);
  }
  if (reportPath !== null) {
    bunTestArgs.push("--reporter=junit", `--reporter-outfile=${reportPath}`);
  }

  const gateArgs =
    reportPath === null ? null : namePattern === null ? [reportPath] : ["--name-pattern", namePattern, reportPath];

  return { bunTestArgs, reportPath, gateArgs, suite };
}

// Same contract the retired vitest.worker-count.ts had: unset -> fallback,
// anything but a positive integer -> hard error.
function resolveWorkerCount(name, fallback, envLike) {
  const raw = envLike[name];
  if (!raw) return fallback;
  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed < 1) {
    throw new Error(`${name} must be a positive integer, got ${JSON.stringify(raw)}`);
  }
  return parsed;
}

/**
 * `bun test --parallel` may only run on a bun with the crashed-worker fix;
 * older buns could report a crashed parallel worker as a passing shard.
 */
export function assertParallelBunFloor(bunTestArgs, bunVersion) {
  const usesParallel = bunTestArgs.some((arg) => arg === "--parallel" || arg.startsWith("--parallel="));
  if (!usesParallel) return;
  const parsed = bunVersion === undefined ? null : /^(\d+)\.(\d+)\.(\d+)/.exec(bunVersion);
  const meetsFloor =
    parsed !== null &&
    (() => {
      const [major, minor, patch] = [Number(parsed[1]), Number(parsed[2]), Number(parsed[3])];
      const [floorMajor, floorMinor, floorPatch] = PARALLEL_SAFE_BUN_FLOOR.split(".").map(Number);
      if (major !== floorMajor) return major > floorMajor;
      if (minor !== floorMinor) return minor > floorMinor;
      return patch >= floorPatch;
    })();
  if (!meetsFloor) {
    throw new Error(
      `user-bun-test: --parallel requires bun >= ${PARALLEL_SAFE_BUN_FLOOR} (crashed-worker fix); ` +
        `executing runtime reports ${bunVersion ?? "no bun version"}`
    );
  }
}
