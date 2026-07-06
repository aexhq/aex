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
    process.exit(1);
  }

  await buildConformance();

  if (!tarball && !version) {
    try {
      const packed = await packCurrentSdk();
      env.AEX_USER_TEST_TARBALL = packed;
    } catch (error) {
      if (packDir) rmSync(packDir, { recursive: true, force: true });
      throw error;
    }
  }

  const vitestArgs = process.argv.slice(2);
  const invocation = buildUserVitestSpawnInvocation(vitestArgs);
  const child = spawn(invocation.command, invocation.args, {
    cwd: appRoot,
    env,
    stdio: "inherit",
    ...invocation.options
  });

  child.on("close", (code, signal) => {
    if (packDir) rmSync(packDir, { recursive: true, force: true });
    if (signal) {
      console.error(`vitest exited with signal ${signal}`);
      process.exit(1);
    }
    process.exit(code ?? 1);
  });
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
  await main();
}
