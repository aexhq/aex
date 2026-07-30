import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { assertParallelBunFloor, prepareLaneInvocation } from "./lane-invocation.mjs";

export { assertParallelBunFloor, prepareLaneInvocation } from "./lane-invocation.mjs";

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(appRoot, "..", "..");
const noSkipsGate = join(repoRoot, "scripts", "cicd", "assert-no-skips.mjs");
const thisFile = fileURLToPath(import.meta.url);
const env = { ...process.env };
let packRoot;

export async function main() {
  const lane = prepareLaneInvocation(process.argv.slice(2), env);
  await prepareArtifacts();
  let bunTestArgs = lane.bunTestArgs;
  if (lane.suite) {
    const selected = lane.suite === "sweep"
      ? collectDefaultSweepFiles()
      : await collectSmokeFiles();
    bunTestArgs = [...selected, ...bunTestArgs];
  }
  assertParallelBunFloor(bunTestArgs, process.versions.bun);

  if (lane.reportPath) {
    const report = resolve(appRoot, lane.reportPath);
    rmSync(report, { force: true });
    mkdirSync(dirname(report), { recursive: true });
  }

  const runRoot = createUserTestTempRoot();
  env.AEX_USER_TEST_TEMP_ROOT = runRoot;
  const invocation = buildUserBunTestSpawnInvocation(bunTestArgs);
  let outcome;
  let failure;
  try {
    outcome = await spawnUserBunTest(invocation);
    if (outcome.error) failure = outcome.error;
  } catch (error) {
    failure = error;
  }

  const cleanupFailures = [];
  try {
    cleanupUserTestTempRoot(runRoot, {
      allowRetainedEntries: Boolean(outcome?.cancellationSignal)
    });
  } catch (error) {
    cleanupFailures.push(error);
  }
  if (packRoot) {
    try {
      removeOwnedDirectory(packRoot, "artifact pack directory");
    } catch (error) {
      cleanupFailures.push(error);
    }
  }
  outcome?.disposeSignalHandlers();

  if (failure || cleanupFailures.length > 0) {
    const failures = [failure, ...cleanupFailures].filter(Boolean);
    throw failures.length === 1
      ? failures[0]
      : new AggregateError(failures, "user-tests: run and cleanup failures occurred");
  }
  if (outcome?.cancellationSignal || outcome?.signal) return 1;
  if ((outcome?.code ?? 1) !== 0) return outcome?.code ?? 1;
  if (lane.gateArgs) return await runNoSkipsGate(lane.gateArgs);
  return 0;
}

async function prepareArtifacts() {
  const selectors = [
    env.AEX_USER_TEST_SDK_TARBALL,
    env.AEX_USER_TEST_CLI_TARBALL,
    env.AEX_USER_TEST_SDK_VERSION,
    env.AEX_USER_TEST_CLI_VERSION
  ].filter((value) => value);
  if (selectors.length === 0) {
    const packed = await packCurrentArtifacts();
    env.AEX_USER_TEST_SDK_TARBALL = packed.sdk;
    env.AEX_USER_TEST_CLI_TARBALL = packed.cli;
  }
}

async function packCurrentArtifacts() {
  packRoot = mkdtempSync(join(tmpdir(), "aex-user-test-pack-"));
  await run(getBunCommand(), ["run", "build"], { cwd: repoRoot, timeoutMs: 300_000 });
  const sdk = await packPackage("sdk", /^aexhq-sdk-.*\.tgz$/);
  const cli = await packPackage("cli", /^aexhq-cli-.*\.tgz$/);
  return { sdk, cli };
}

async function packPackage(name, pattern) {
  await run(
    getBunCommand(),
    ["pm", "pack", "--destination", packRoot, "--ignore-scripts"],
    { cwd: join(repoRoot, "packages", name), timeoutMs: 180_000 }
  );
  const matches = readdirSync(packRoot).filter((file) => pattern.test(file));
  if (matches.length !== 1) throw new Error(`expected one packed ${name} tarball, found ${matches.length}`);
  return join(packRoot, matches[0]);
}

export function collectDefaultSweepFiles() {
  const files = [];
  const walk = (relative) => {
    for (const entry of readdirSync(join(appRoot, relative), { withFileTypes: true })) {
      const path = `${relative}/${entry.name}`;
      if (entry.isDirectory()) {
        if (entry.name !== "node_modules" && entry.name !== "e2e") walk(path);
      } else if (entry.isFile() && entry.name.endsWith(".test.ts")) {
        files.push(path);
      }
    }
  };
  walk("test");
  files.sort();
  if (files.length === 0) throw new Error("user-bun-test: default sweep collected zero test files");
  return files;
}

async function collectSmokeFiles() {
  const { PUBLISHED_ARTIFACT_SMOKE_FILES } = await import(
    new URL("../test/_fixtures/smoke-suite.ts", import.meta.url).href
  );
  const files = Object.values(PUBLISHED_ARTIFACT_SMOKE_FILES);
  if (files.length === 0) throw new Error("user-bun-test: smoke manifest lists zero files");
  for (const file of files) {
    if (!existsSync(join(appRoot, file))) {
      throw new Error(`user-bun-test: smoke manifest file does not exist: ${file}`);
    }
  }
  return files;
}

function runNoSkipsGate(args) {
  return spawnForCode(buildSpawnInvocation(getBunCommand(), [noSkipsGate, ...args]), appRoot);
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
    const forward = (signal) => {
      if (cancellationSignal) return;
      cancellationSignal = signal;
      try {
        child.kill(signal);
      } catch (error) {
        childError = error;
      }
    };
    const onSigint = () => forward("SIGINT");
    const onSigterm = () => forward("SIGTERM");
    const disposeSignalHandlers = () => {
      process.off("SIGINT", onSigint);
      process.off("SIGTERM", onSigterm);
    };
    process.on("SIGINT", onSigint);
    process.on("SIGTERM", onSigterm);
    child.once("error", (error) => { childError ??= error; });
    child.once("close", (code, signal) => {
      resolvePromise({ code, signal, cancellationSignal, error: childError, disposeSignalHandlers });
    });
  });
}

function spawnForCode(invocation, cwd) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(invocation.command, invocation.args, {
      cwd,
      env,
      stdio: "inherit",
      ...invocation.options
    });
    child.once("error", reject);
    child.once("close", (code, signal) => {
      if (signal) reject(new Error(`child exited with signal ${signal}`));
      else resolvePromise(code ?? 1);
    });
  });
}

export function createUserTestTempRoot(parent = tmpdir()) {
  return mkdtempSync(join(parent, "aex-user-test-run-"));
}

export function cleanupUserTestTempRoot(runRoot, options = {}) {
  let entries = [];
  try {
    entries = readdirSync(runRoot);
  } catch (error) {
    throw new Error(`user-tests: failed to inspect owned run temp root ${runRoot}: ${describeFsError(error)}`);
  }
  removeOwnedDirectory(runRoot, "owned run temp root");
  if (entries.length > 0) {
    const message =
      `user-tests: fixture cleanup left ${entries.length} ${entries.length === 1 ? "entry" : "entries"} ` +
      `in ${runRoot}: ${entries.slice(0, 20).join(", ")}; removed the owned run root`;
    if (options.allowRetainedEntries) {
      console.error(message.replace("fixture cleanup left", "graceful cancellation retained"));
    } else {
      throw new Error(message);
    }
  }
}

function removeOwnedDirectory(path, label) {
  rmSync(path, { recursive: true, force: true });
  if (existsSync(path)) throw new Error(`user-tests: failed to remove ${label} ${path}`);
}

export function buildUserBunTestSpawnInvocation(args, command = getBunCommand()) {
  return buildSpawnInvocation(command, ["test", ...args]);
}

export function buildSpawnInvocation(command, args, spawnEnv = env) {
  if (process.platform === "win32" && /\.(?:cmd|bat)$/i.test(command)) {
    return {
      command: spawnEnv.ComSpec ?? "cmd.exe",
      args: ["/d", "/s", "/c", windowsCommandLine(command, args)],
      options: { shell: false, windowsVerbatimArguments: true }
    };
  }
  return { command, args: [...args], options: { shell: false } };
}

function windowsCommandLine(command, args) {
  return `"${[command, ...args].map(quoteWindowsCommandArg).join(" ")}"`;
}

function quoteWindowsCommandArg(value) {
  if (value.length === 0) return '""';
  return `"${value.replace(/(\\*)"/g, '$1$1\\"').replace(/(\\+)$/g, "$1$1")}"`;
}

function run(command, args, options) {
  return new Promise((resolvePromise, reject) => {
    let stdout = "";
    let stderr = "";
    const invocation = buildSpawnInvocation(command, args);
    const child = spawn(invocation.command, invocation.args, {
      cwd: options.cwd,
      env,
      stdio: ["ignore", "pipe", "pipe"],
      ...invocation.options
    });
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`timed out after ${options.timeoutMs}ms: ${command} ${args.join(" ")}`));
    }, options.timeoutMs);
    child.stdout?.on("data", (chunk) => { stdout += chunk.toString(); });
    child.stderr?.on("data", (chunk) => { stderr += chunk.toString(); });
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      if (code === 0) resolvePromise();
      else reject(new Error(`${command} ${args.join(" ")} exited ${code}\n${stdout}\n${stderr}`));
    });
  });
}

function describeFsError(error) {
  return error instanceof Error ? error.message : String(error);
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
