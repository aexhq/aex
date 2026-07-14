import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(here, "..");
const repoRoot = resolve(appRoot, "..", "..");
const sdkRoot = join(repoRoot, "packages", "sdk");
const generatedDistLockScript = join(repoRoot, "scripts", "with-generated-dist-lock.mjs");
const thisFile = fileURLToPath(import.meta.url);

const env = { ...process.env };
let packDir;

export async function main() {
  const tarball = env.AEX_USER_TEST_TARBALL;
  const version = env.AEX_USER_TEST_VERSION;

  if (tarball && version) {
    console.error("AEX_USER_TEST_TARBALL and AEX_USER_TEST_VERSION are mutually exclusive.");
    return 1;
  }

  await buildConformance();

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

  const vitestArgs = process.argv.slice(2);
  const invocation = buildUserVitestSpawnInvocation(vitestArgs);
  const runTempRoot = createUserTestTempRoot();
  env.AEX_USER_TEST_TEMP_ROOT = runTempRoot;

  let outcome;
  let runFailure;
  try {
    outcome = await spawnUserVitest(invocation);
  } catch (error) {
    runFailure = error;
  }

  const cleanupFailures = [];
  try {
    cleanupUserTestTempRoot(runTempRoot);
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

  if (runFailure || cleanupFailures.length > 0) {
    const failures = [runFailure, ...cleanupFailures].filter((error) => error !== undefined);
    throw failures.length === 1
      ? failures[0]
      : new AggregateError(failures, "user-tests: runner and cleanup failures occurred");
  }
  if (outcome.signal) {
    console.error(`vitest exited with signal ${outcome.signal}`);
    return 1;
  }
  return outcome.code ?? 1;
}

function spawnUserVitest(invocation) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(invocation.command, invocation.args, {
      cwd: appRoot,
      env,
      stdio: "inherit",
      ...invocation.options
    });
    child.once("error", reject);
    child.once("close", (code, signal) => resolvePromise({ code, signal }));
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
export function cleanupUserTestTempRoot(runTempRoot) {
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

export function buildUserVitestSpawnInvocation(vitestArgs, command = getBunCommand()) {
  return buildSpawnInvocation(command, ["run", "vitest", "run", ...vitestArgs]);
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

async function buildConformance() {
  await run(getBunCommand(), ["run", "--cwd", repoRoot, "--filter", "@aexhq/conformance", "build"], {
    cwd: repoRoot,
    timeoutMs: 120_000
  });
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
