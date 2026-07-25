/**
 * Lane runner for the layer-4 user tests: packs/selects the artifact under
 * test, owns the per-run temp root, then spawns `bun test` with the lane's
 * argv (the successor of user-vitest.mjs — the tarball/temp-root logic is
 * runner-agnostic; only the spawn argv and the lane flags changed).
 *
 * Wrapper-owned flags (everything else is forwarded to `bun test` verbatim):
 *   --report=<path>        write a junit report there and run
 *                          scripts/cicd/assert-no-skips.mjs on it after a
 *                          green run; a bun `-t <pattern>` in the forwarded
 *                          args is mirrored into the gate's --name-pattern
 *                          (bun marks every non-selected test <skipped/>).
 *   --parallel-env=NAME:N  inject `--parallel=<value of env NAME, default N>`
 *                          (positive integer, else hard error — the same
 *                          contract the retired vitest.worker-count.ts had).
 *                          Gated on bun >= 1.3.14 (crashed-worker fix).
 *   --sweep | --smoke      expand the default-sweep / published-artifact-smoke
 *                          file list (see below) instead of explicit files.
 *
 * A run with NO file selection is refused: a bare `bun test` in this package
 * would collect EVERY live suite (admission-gates, heavy, e2e, ...) — an
 * accidental full-spend sweep.
 *
 * Lane concurrency mapping (vitest -> bun), derived spend bound per lane.
 * The vitest bound was maxWorkers x maxConcurrency; bun runs tests within a
 * file serially (no sequence.concurrent analog), so the bound maps to
 * `--parallel=N` worker processes with serial in-file execution:
 *
 *   lane            vitest workersxconc      bun expression            bound
 *   test:user       env(MAX_WORKERS,2) x 1   --parallel=env(2), serial  2
 *   test:user:files same config, 1 CI file   --parallel=env(2), serial  1/job
 *   test:user:smoke env(MAX_WORKERS,2) x 1   --parallel=env(2), serial  2
 *   test:user:offline env(OFFLINE,4) x 1     --parallel=env(4), serial  4 (no spend)
 *   admission/heavy/providers/fuzz/e2e: serial file(s), serial tests    1
 *   test:user:tool-fuzz 1 file x env(TOOL_FUZZ_CONCURRENCY,4) in-file   was 4,
 *     now SERIAL (bound 1): bun cannot run unmarked tests concurrently
 *     within one file, so the in-file concurrency lever is gone until the
 *     cells are marked test.concurrent (follow-up); the
 *     AEX_USER_TEST_TOOL_FUZZ_CONCURRENCY knob is inert for now and the lane
 *     trades wall-clock (up to ~4x) for a strictly lower spend bound.
 *
 * Live validation of these mappings on the dev plane is explicitly OWED /
 * deferred (user decision): no live lane was executed as part of the runner
 * flip. Validate lane-by-lane with spend monitoring before trusting the
 * mapping in release wiring.
 */
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { LIVE_TEST_SHARD_CONFIG, collectTestFiles } from "./shard-files.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(here, "..");
const repoRoot = resolve(appRoot, "..", "..");
const sdkRoot = join(repoRoot, "packages", "sdk");
const generatedDistLockScript = join(repoRoot, "scripts", "with-generated-dist-lock.mjs");
const noSkipsGateScript = join(repoRoot, "scripts", "cicd", "assert-no-skips.mjs");
const thisFile = fileURLToPath(import.meta.url);

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

const env = { ...process.env };
let packDir;

export async function main() {
  const tarball = env.AEX_USER_TEST_TARBALL;
  const version = env.AEX_USER_TEST_VERSION;

  if (tarball && version) {
    console.error("AEX_USER_TEST_TARBALL and AEX_USER_TEST_VERSION are mutually exclusive.");
    return 1;
  }

  const lane = prepareLaneInvocation(process.argv.slice(2), env);

  await buildHarnessPackages();

  if (!tarball && !version) {
    try {
      const packed = await packCurrentSdk();
      env.AEX_USER_TEST_TARBALL = packed;
    } catch (error) {
      if (packDir) {
        try {
          removeOwnedDirectory(packDir, "SDK pack tempdir");
        } catch (cleanupError) {
          throw new AggregateError(
            [error, cleanupError],
            "user-tests: SDK packing failed and its tempdir could not be removed"
          );
        }
      }
      throw error;
    }
  }

  let bunTestArgs = lane.bunTestArgs;
  if (lane.suite) {
    const suiteFiles = lane.suite === "sweep" ? collectDefaultSweepFiles() : await collectSmokeFiles();
    bunTestArgs = [...suiteFiles, ...bunTestArgs];
  }
  assertParallelBunFloor(bunTestArgs, process.versions.bun);

  if (lane.reportPath) {
    const reportAbs = resolve(appRoot, lane.reportPath);
    // A stale report from an earlier run must never satisfy the gate: bun
    // writes NO outfile at all when it collects zero tests.
    rmSync(reportAbs, { force: true });
    mkdirSync(dirname(reportAbs), { recursive: true });
  }

  const invocation = buildUserBunTestSpawnInvocation(bunTestArgs);
  const runTempRoot = createUserTestTempRoot();
  env.AEX_USER_TEST_TEMP_ROOT = runTempRoot;
  // C4: one fragment directory per lane, emptied first so the aggregate below
  // describes THIS run and cannot inherit an earlier one's coverage.
  const wireConformanceDir = join(appRoot, ".tmp", "wire-conformance");
  rmSync(wireConformanceDir, { recursive: true, force: true });
  mkdirSync(wireConformanceDir, { recursive: true });
  env.AEX_WIRE_CONFORMANCE_DIR = wireConformanceDir;

  let outcome;
  let runFailure;
  try {
    outcome = await spawnUserBunTest(invocation);
    if (outcome.error) runFailure = outcome.error;
  } catch (error) {
    runFailure = error;
  }

  const cleanupFailures = [];
  try {
    cleanupUserTestTempRoot(runTempRoot, {
      allowRetainedEntries: Boolean(outcome?.cancellationSignal)
    });
  } catch (error) {
    cleanupFailures.push(error);
  }
  if (packDir) {
    try {
      removeOwnedDirectory(packDir, "SDK pack tempdir");
    } catch (error) {
      cleanupFailures.push(error);
    }
  }
  outcome?.disposeSignalHandlers();

  if (runFailure || cleanupFailures.length > 0) {
    const failures = [runFailure, ...cleanupFailures].filter((error) => error !== undefined);
    throw failures.length === 1
      ? failures[0]
      : new AggregateError(failures, "user-tests: runner and cleanup failures occurred");
  }
  if (outcome.cancellationSignal) {
    console.error(`user-tests: exiting after graceful ${outcome.cancellationSignal} cancellation`);
    return 1;
  }
  if (outcome.signal) {
    console.error(`bun test exited with signal ${outcome.signal}`);
    return 1;
  }
  // C4 runs whatever the lane's exit code was: a failing lane still produced
  // real bytes, and the coverage picture is exactly what a reader needs in order
  // to know how much of the surface that run actually covered.
  const wireConformanceCode = await reportWireConformance(wireConformanceDir);

  if ((outcome.code ?? 1) !== 0) return outcome.code ?? 1;
  if (wireConformanceCode !== 0) return wireConformanceCode;

  if (lane.gateArgs) {
    const gateCode = await runNoSkipsGate(lane.gateArgs);
    if (gateCode !== 0) return gateCode;
  }
  return 0;
}

/**
 * C4 — the lane's wire-conformance verdict.
 *
 * Per-file failures already happen in `test/preload.ts`; this is the only place
 * that can state COVERAGE, because coverage is a property of the run and no
 * single test process sees more than a handful of routes. Per `04-gates.md`,
 * both halves are printed unconditionally — what could have been checked
 * (`formatResponseSchemaCoverage`) and what actually was
 * (`formatWireConformanceReport`) — so a green lane over a dozen routes cannot
 * be read as a verified surface.
 */
async function reportWireConformance(wireConformanceDir) {
  const { readWireConformanceFragments } = await import(
    join(appRoot, "test", "_fixtures", "wire-conformance.ts")
  );
  const { formatResponseSchemaCoverage, formatWireConformanceReport } = await import(
    "@aexhq/contracts/testing"
  );
  const harvest = readWireConformanceFragments(wireConformanceDir);

  console.error("");
  console.error("=== C4 wire conformance ===");
  console.error(formatResponseSchemaCoverage());
  for (const reason of harvest.notArmed) {
    console.error(`  NOT ARMED: ${reason}`);
  }
  if (harvest.fragments === 0) {
    // Never read as a pass. A lane may legitimately observe nothing, but saying
    // so out loud is the difference between "nothing was checked" and "checked
    // and clean" — and only the first of those is true here.
    console.error(
      "wire conformance: NO PROCESS REPORTED. Nothing was checked against the wire in this lane."
    );
    return 0;
  }
  console.error(
    `wire conformance: folded ${harvest.reports} report(s) from ${harvest.fragments} process(es)`
  );
  console.error(formatWireConformanceReport(harvest.report));
  for (const failure of harvest.armFailures) {
    console.error(`  ARM FAILURE: ${failure}`);
  }

  if (harvest.armFailures.length > 0) {
    console.error(
      `C4 FAILED: ${harvest.armFailures.length} process(es) could not attach the harness — ` +
        `their responses were never checked.`
    );
    return 1;
  }
  if (harvest.report.violations.length > 0) {
    console.error(`C4 FAILED: ${harvest.report.violations.length} wire-conformance violation(s).`);
    return 1;
  }
  return 0;
}

/**
 * Pure lane-argv processing: split wrapper-owned flags from the `bun test`
 * argv, resolve --parallel-env, and derive the junit + no-skips gate wiring.
 * Throws on any ambiguous or selection-free invocation.
 */
export function prepareLaneInvocation(argv, envLike = env) {
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

/**
 * The default `test:user` sweep: every test/**\/*.test.ts EXCEPT the dedicated
 * explicit gates, exactly as the retired vitest.config.ts include/exclude pair
 * collected. Kept glob-driven (not a hardcoded list) so a NEW test file lands
 * in the sweep automatically — a silent drop from the sweep is dead coverage.
 *
 * Dedicated explicit lanes stay out of the sweep so they never run implicitly:
 *   - edge-admission-gates (cap-saturating; isolated low-cap workspace lane);
 *   - live-sdk-heavy-session (run only AFTER the rest pass);
 *   - live-api-fuzz + live-sdk-tool-capability-fuzz (own paid gates);
 *   - test/live/providers/** (on-demand; keeps non-gate provider billing out
 *     of the release gate);
 *   - test/e2e/** (own account-PAT bootstrap; must never run implicitly).
 */
export function collectDefaultSweepFiles() {
  // Live files: reuse the CI shard manifest so the sweep and the per-file CI
  // matrix can never disagree about lane exclusions.
  const liveFiles = collectTestFiles();

  // Non-live files (offline/, _fixtures/, any future sibling): walk test/
  // minus the directories that are dedicated lanes or not test trees.
  const skippedDirectories = new Set(["live", "e2e", "node_modules"]);
  const nonLiveFiles = [];
  const walk = (rel) => {
    for (const entry of readdirSync(join(appRoot, rel), { withFileTypes: true })) {
      const relPath = `${rel}/${entry.name}`;
      if (entry.isDirectory()) {
        if (!(rel === "test" && skippedDirectories.has(entry.name)) && entry.name !== "node_modules") {
          walk(relPath);
        }
      } else if (entry.name.endsWith(".test.ts")) {
        nonLiveFiles.push(relPath);
      }
    }
  };
  walk("test");

  const files = [...liveFiles, ...nonLiveFiles].sort();
  if (files.length === 0) throw new Error("user-bun-test: default sweep collected zero test files");
  return files;
}

async function collectSmokeFiles() {
  const manifestUrl = new URL("../test/_fixtures/smoke-suite.ts", import.meta.url);
  const { PUBLISHED_ARTIFACT_SMOKE_FILES } = await import(manifestUrl.href);
  const files = Object.values(PUBLISHED_ARTIFACT_SMOKE_FILES);
  if (files.length === 0) throw new Error("user-bun-test: smoke manifest lists zero files");
  for (const file of files) {
    if (!existsSync(join(appRoot, file))) {
      throw new Error(`user-bun-test: smoke manifest file does not exist: ${file}`);
    }
  }
  return files;
}

async function runNoSkipsGate(gateArgs) {
  return await new Promise((resolvePromise, reject) => {
    const invocation = buildSpawnInvocation(getBunCommand(), [noSkipsGateScript, ...gateArgs]);
    let child;
    try {
      child = spawn(invocation.command, invocation.args, {
        cwd: appRoot,
        env,
        stdio: "inherit",
        ...invocation.options
      });
    } catch (error) {
      reject(error);
      return;
    }
    child.once("error", reject);
    child.once("close", (code, signal) => {
      if (signal) {
        reject(new Error(`user-bun-test: assert-no-skips gate exited with signal ${signal}`));
        return;
      }
      resolvePromise(code ?? 1);
    });
  });
}

function spawnUserBunTest(invocation) {
  return new Promise((resolvePromise, reject) => {
    let child;
    try {
      child = spawn(invocation.command, invocation.args, {
        cwd: appRoot,
        env,
        stdio: "inherit",
        ...invocation.options
      });
    } catch (error) {
      reject(error);
      return;
    }

    let cancellationSignal;
    let childError;
    const forwardSignal = (signal) => {
      if (cancellationSignal) return;
      cancellationSignal = signal;
      console.error(`user-tests: received ${signal}; forwarding it to the bun test child and waiting for close`);
      try {
        if (!child.kill(signal) && child.exitCode === null && child.signalCode === null) {
          childError = new Error(`user-tests: failed to forward ${signal} to the bun test child`);
        }
      } catch (error) {
        childError = new Error(`user-tests: failed to forward ${signal} to the bun test child: ${describeFsError(error)}`);
      }
    };
    const onSigint = () => forwardSignal("SIGINT");
    const onSigterm = () => forwardSignal("SIGTERM");
    const removeSignalHandlers = () => {
      process.off("SIGINT", onSigint);
      process.off("SIGTERM", onSigterm);
    };

    process.on("SIGINT", onSigint);
    process.on("SIGTERM", onSigterm);
    child.once("error", (error) => {
      childError ??= error;
    });
    child.once("close", (code, signal) => {
      resolvePromise({ code, signal, cancellationSignal, error: childError, disposeSignalHandlers: removeSignalHandlers });
    });
  });
}

export function createUserTestTempRoot(parentDir = tmpdir()) {
  return mkdtempSync(join(parentDir, "aex-user-test-run-"));
}

/**
 * Remove the temp root owned by one wrapper invocation. Any retained child
 * is reported after removal so a worker cleanup regression fails the run
 * without preserving hardlinks in the global Bun cache.
 */
export function cleanupUserTestTempRoot(runTempRoot, options = {}) {
  let retainedEntries;
  let inspectionFailure;
  try {
    retainedEntries = readdirSync(runTempRoot);
  } catch (error) {
    retainedEntries = [];
    inspectionFailure = new Error(
      `user-tests: failed to inspect owned run temp root ${runTempRoot}: ${describeFsError(error)}`
    );
  }

  try {
    removeOwnedDirectory(runTempRoot, "owned run temp root");
  } catch (removalFailure) {
    if (inspectionFailure) {
      throw new AggregateError(
        [inspectionFailure, removalFailure],
        "user-tests: failed to inspect and remove the owned run temp root"
      );
    }
    throw removalFailure;
  }

  if (inspectionFailure) throw inspectionFailure;

  if (retainedEntries.length > 0) {
    const shown = retainedEntries.slice(0, 20).join(", ");
    const omitted = retainedEntries.length > 20 ? ` (+${retainedEntries.length - 20} more)` : "";
    const noun = retainedEntries.length === 1 ? "entry" : "entries";
    if (options.allowRetainedEntries) {
      console.error(
        `user-tests: graceful cancellation retained ${retainedEntries.length} ${noun} in ${runTempRoot}: ` +
          `${shown}${omitted}; removed the owned run root`
      );
      return;
    }
    throw new Error(
      `user-tests: fixture cleanup left ${retainedEntries.length} ${noun} in ${runTempRoot}: ` +
        `${shown}${omitted}; removed the owned run root and failing the run`
    );
  }
}

function removeOwnedDirectory(path, label) {
  try {
    rmSync(path, { recursive: true, force: true });
  } catch (error) {
    throw new Error(`user-tests: failed to remove ${label} ${path}: ${describeFsError(error)}`);
  }
  if (existsSync(path)) {
    throw new Error(`user-tests: remove returned successfully but ${label} still exists: ${path}`);
  }
}

function describeFsError(error) {
  if (!(error instanceof Error)) return String(error);
  const details = [error.code, error.syscall, error.path].filter(
    (value) => typeof value === "string" && value.length > 0
  );
  return details.length > 0 ? `${details.join(" ")}: ${error.message}` : error.message;
}

export function buildUserBunTestSpawnInvocation(bunTestArgs, command = getBunCommand()) {
  return buildSpawnInvocation(command, ["test", ...bunTestArgs]);
}

export function buildSpawnInvocation(command, args, spawnEnv = env) {
  if (isWindowsCommandShim(command)) {
    return {
      command: spawnEnv.ComSpec ?? "cmd.exe",
      args: ["/d", "/s", "/c", windowsCommandLine(command, args)],
      options: { shell: false, windowsVerbatimArguments: true }
    };
  }

  return {
    command,
    args: [...args],
    options: { shell: false }
  };
}

function isWindowsCommandShim(command) {
  return process.platform === "win32" && /\.(?:cmd|bat)$/i.test(command);
}

function windowsCommandLine(command, args) {
  const argv = [command, ...args].map(quoteWindowsCommandArg).join(" ");
  return `"${argv}"`;
}

function quoteWindowsCommandArg(value) {
  if (value.length === 0) return '""';
  return `"${value.replace(/(\\*)"/g, '$1$1\\"').replace(/(\\+)$/g, "$1$1")}"`;
}

// The harness process itself imports @aexhq/contracts/testing
// (waitForCondition) from the workspace, so it must be built before
// `bun test` collects any file.
async function buildHarnessPackages() {
  await run(
    getBunCommand(),
    ["run", "--cwd", repoRoot, "--filter", "@aexhq/contracts", "build"],
    {
      cwd: repoRoot,
      timeoutMs: 240_000
    }
  );
}

async function packCurrentSdk() {
  packDir = mkdtempSync(join(tmpdir(), "aex-user-test-sdk-pack-"));
  await run(getBunCommand(), [generatedDistLockScript, "bun", "pm", "pack", "--destination", packDir], {
    cwd: sdkRoot,
    timeoutMs: 180_000
  });

  const tarballs = readdirSync(packDir).filter((name) => /^aexhq-sdk-.*\.tgz$/.test(name));
  if (tarballs.length !== 1) {
    throw new Error(`expected one packed @aexhq/sdk tarball in ${packDir}, found ${tarballs.length}`);
  }
  const packed = join(packDir, tarballs[0]);
  if (!existsSync(packed)) throw new Error(`packed tarball does not exist: ${packed}`);
  return packed;
}

async function run(command, args, options) {
  const timeoutMs = options.timeoutMs ?? 60_000;
  await new Promise((resolvePromise, reject) => {
    let stdout = "";
    let stderr = "";
    const invocation = buildSpawnInvocation(command, args);
    const childProcess = spawn(invocation.command, invocation.args, {
      cwd: options.cwd,
      env,
      stdio: ["ignore", "pipe", "pipe"],
      ...invocation.options
    });
    const timer = setTimeout(() => {
      childProcess.kill("SIGKILL");
      reject(new Error(`timed out after ${timeoutMs}ms: ${command} ${args.join(" ")}`));
    }, timeoutMs);
    childProcess.stdout?.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    childProcess.stderr?.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    childProcess.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    childProcess.on("close", (code) => {
      clearTimeout(timer);
      if (code === 0) {
        resolvePromise();
        return;
      }
      reject(
        new Error(
          `${command} ${args.join(" ")} exited with code ${code ?? -1}\n` +
            `--- stdout ---\n${stdout}\n--- stderr ---\n${stderr}`
        )
      );
    });
  });
}

function getBunCommand() {
  if (env.AEX_USER_TEST_BUN) return env.AEX_USER_TEST_BUN;
  if (env.BUN) return env.BUN;
  if ("bun" in process.versions) return process.execPath;
  return process.platform === "win32" ? "bun.exe" : "bun";
}

if (process.argv[1] && resolve(process.argv[1]) === thisFile) {
  try {
    process.exitCode = await main();
  } catch (error) {
    console.error(error instanceof Error ? error.stack : error);
    process.exitCode = 1;
  }
}
