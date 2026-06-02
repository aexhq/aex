/**
 * Shared install fixture for the user-tests layer.
 *
 * Each scenario calls {@link installAntpath} which:
 *   1. Reads ANTPATH_USER_TEST_TARBALL or ANTPATH_USER_TEST_VERSION
 *      (explicit overrides stay mutually exclusive). When neither is
 *      set, local/offline runs pack the current workspace SDK once and
 *      install that tarball.
 *   2. Creates a fresh tempdir.
 *   3. Materializes a minimal package.json there.
 *   4. Runs `npm install <tarball|antpath@version>` against it.
 *   5. Returns paths for the install so scenarios can spawn child
 *      processes with cwd = installDir.
 *
 * Cleanup is the test's responsibility (typically in afterAll).
 *
 * Why a fixture file instead of vitest globalSetup: each scenario
 * needs its own clean install to avoid cross-test mutation (e.g. the
 * TS consumer scenario adds devDependencies). Sharing a single
 * install would leak state.
 */
import { spawn, type SpawnOptions } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export interface InstallResult {
  /** Absolute path to the install tempdir (npm install was run here). */
  readonly installDir: string;
  /** `node_modules/antpath` inside the install tempdir. */
  readonly antpathDir: string;
  /** `node_modules/antpath/package.json` parsed. */
  readonly antpathPackageJson: AntpathPackageJson;
  /** Resolved version of antpath that was installed. */
  readonly resolvedVersion: string;
  /** Spec passed to npm install (tarball path or `antpath@<version>`). */
  readonly installSpec: string;
  /** Source kind for diagnostics. */
  readonly source: "tarball" | "registry" | "local-pack";
  /** Run cleanup: removes installDir recursively. Safe to call twice. */
  readonly cleanup: () => void;
}

export interface AntpathPackageJson {
  readonly name: string;
  readonly version: string;
  readonly type?: string;
  readonly main?: string;
  readonly types?: string;
  readonly bin?: Record<string, string>;
  readonly exports?: unknown;
  readonly engines?: Record<string, string>;
  readonly dependencies?: Record<string, string>;
}

export interface InstallOptions {
  /**
   * Override the npm registry. Tests against the post-publish registry
   * pass undefined; tests against a local verdaccio could pass a URL.
   */
  readonly registryUrl?: string;
  /** Override the install timeout (ms). Default 120s. */
  readonly timeoutMs?: number;
}

export interface ResolveInstallSpecOptions {
  /** Environment map to read. Defaults to process.env. */
  readonly env?: NodeJS.ProcessEnv;
  /** Filesystem existence probe. Defaults to existsSync. */
  readonly pathExists?: (path: string) => boolean;
  /** Local SDK packer. Defaults to the cached workspace packer. */
  readonly packLocalSdk?: () => Promise<string>;
}

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..", "..");
const packLockDir = join(
  tmpdir(),
  `antpath-user-test-sdk-pack-${createHash("sha256").update(repoRoot).digest("hex").slice(0, 16)}.lock`
);
let localSdkPackPromise: Promise<string> | null = null;

/**
 * Resolve which artifact the runner is testing.
 * Exported separately so the diagnostic header can print it before
 * actually running npm install.
 */
export async function resolveInstallSpec(
  options: ResolveInstallSpecOptions = {}
): Promise<{ readonly spec: string; readonly source: InstallResult["source"] }> {
  const env = options.env ?? process.env;
  const pathExists = options.pathExists ?? existsSync;
  const packLocalSdk = options.packLocalSdk ?? packCurrentSdkOnce;
  const tarball = env["ANTPATH_USER_TEST_TARBALL"];
  const version = env["ANTPATH_USER_TEST_VERSION"];
  if (tarball && version) {
    throw new Error(
      "user-tests: ANTPATH_USER_TEST_TARBALL and ANTPATH_USER_TEST_VERSION are mutually exclusive. Set exactly one."
    );
  }
  if (tarball) {
    if (!pathExists(tarball)) {
      throw new Error(`user-tests: ANTPATH_USER_TEST_TARBALL points at a non-existent path: ${tarball}`);
    }
    return { spec: tarball, source: "tarball" };
  }
  if (version) {
    if (!/^[0-9]+\.[0-9]+\.[0-9]+([\-+][0-9A-Za-z.\-]+)?$/.test(version)) {
      throw new Error(
        `user-tests: ANTPATH_USER_TEST_VERSION must be a concrete semver (e.g. 0.2.1), got: ${version}`
      );
    }
    return { spec: `antpath@${version}`, source: "registry" };
  }
  const localTarball = await packLocalSdk();
  return { spec: localTarball, source: "local-pack" };
}

/**
 * Install the resolved antpath artifact into a fresh tempdir.
 * Throws if npm exits non-zero or installs the wrong version.
 */
export async function installAntpath(options: InstallOptions = {}): Promise<InstallResult> {
  const { spec, source } = await resolveInstallSpec();
  const installDir = mkdtempSync(join(tmpdir(), "antpath-user-test-"));
  // Minimal host package.json so npm install doesn't complain.
  const hostPkg = {
    name: "antpath-user-test-host",
    version: "0.0.0",
    private: true,
    type: "module"
  };
  writeFileSync(join(installDir, "package.json"), JSON.stringify(hostPkg, null, 2));

  const args = ["install", spec, "--no-audit", "--no-fund", "--ignore-scripts"];
  if (options.registryUrl) {
    args.push("--registry", options.registryUrl);
  }

  const timeoutMs = options.timeoutMs ?? 120_000;
  try {
    await runNpm(args, { cwd: installDir }, timeoutMs);
  } catch (error) {
    rmSync(installDir, { recursive: true, force: true });
    throw error;
  }

  const antpathDir = join(installDir, "node_modules", "antpath");
  if (!existsSync(antpathDir)) {
    rmSync(installDir, { recursive: true, force: true });
    throw new Error(`user-tests: install completed but ${antpathDir} is missing`);
  }
  const pkgPath = join(antpathDir, "package.json");
  if (!existsSync(pkgPath)) {
    rmSync(installDir, { recursive: true, force: true });
    throw new Error(`user-tests: install completed but ${pkgPath} is missing`);
  }
  let pkg: AntpathPackageJson;
  try {
    const text = await import("node:fs/promises").then((m) => m.readFile(pkgPath, "utf8"));
    pkg = JSON.parse(text) as AntpathPackageJson;
  } catch (error) {
    rmSync(installDir, { recursive: true, force: true });
    throw new Error(`user-tests: could not read installed package.json: ${(error as Error).message}`);
  }

  if (source === "registry") {
    const want = spec.replace(/^antpath@/, "");
    if (pkg.version !== want) {
      rmSync(installDir, { recursive: true, force: true });
      throw new Error(
        `user-tests: registry resolved antpath@${want} but installed package reports version ${pkg.version}`
      );
    }
  }

  let cleanedUp = false;
  const cleanup = (): void => {
    if (cleanedUp) return;
    cleanedUp = true;
    try {
      rmSync(installDir, { recursive: true, force: true });
    } catch {
      // Best effort: a stuck file handle on Windows is not worth failing for.
    }
  };

  return {
    installDir,
    antpathDir,
    antpathPackageJson: pkg,
    resolvedVersion: pkg.version,
    installSpec: spec,
    source,
    cleanup
  };
}

function packCurrentSdkOnce(): Promise<string> {
  localSdkPackPromise ??= packCurrentSdk();
  return localSdkPackPromise;
}

async function packCurrentSdk(): Promise<string> {
  return await withPackLock(async () => {
    const packDir = mkdtempSync(join(tmpdir(), "antpath-user-test-sdk-pack-"));
    const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
    try {
      await runCommand(pnpm, ["--filter", "antpath", "pack", "--pack-destination", packDir], {
        cwd: repoRoot,
        timeoutMs: 180_000
      }).then((result) => {
        if (result.exitCode !== 0) {
          throw new Error(
            `pnpm --filter antpath pack exited with code ${result.exitCode}\n--- stdout ---\n${result.stdout}\n--- stderr ---\n${result.stderr}`
          );
        }
      });
      const tarballs = readdirSync(packDir).filter((name) => /^antpath-.*\.tgz$/.test(name));
      if (tarballs.length !== 1) {
        throw new Error(`user-tests: expected one packed antpath tarball in ${packDir}, found ${tarballs.length}`);
      }
      return join(packDir, tarballs[0]!);
    } catch (error) {
      rmSync(packDir, { recursive: true, force: true });
      throw error;
    }
  });
}

async function withPackLock<T>(fn: () => Promise<T>): Promise<T> {
  const startedAt = Date.now();
  while (true) {
    try {
      mkdirSync(packLockDir);
      break;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      try {
        if (Date.now() - statSync(packLockDir).mtimeMs > 10 * 60_000) {
          rmSync(packLockDir, { recursive: true, force: true });
          continue;
        }
      } catch {
        // Race with lock release; retry below.
      }
      if (Date.now() - startedAt > 4 * 60_000) {
        throw new Error(`user-tests: timed out waiting for local SDK pack lock at ${packLockDir}`);
      }
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  try {
    return await fn();
  } finally {
    rmSync(packLockDir, { recursive: true, force: true });
  }
}

/**
 * Run a shell command. Returns { stdout, stderr, exitCode }.
 * Rejects on timeout or spawn error; never rejects on non-zero exit
 * — the caller decides whether non-zero is a failure.
 */
export interface RunResult {
  readonly exitCode: number;
  readonly stdout: string;
  readonly stderr: string;
  readonly signal: NodeJS.Signals | null;
}

export async function runCommand(
  command: string,
  args: readonly string[],
  options: SpawnOptions & { readonly timeoutMs?: number } = {}
): Promise<RunResult> {
  const timeoutMs = options.timeoutMs ?? 60_000;
  const { timeoutMs: _omit, ...spawnOptions } = options;
  void _omit;
  return await new Promise<RunResult>((resolve, reject) => {
    let stdout = "";
    let stderr = "";
    const child = spawn(command, args as string[], {
      ...spawnOptions,
      stdio: ["ignore", "pipe", "pipe"],
      shell: process.platform === "win32"
    });
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`timed out after ${timeoutMs}ms: ${command} ${args.join(" ")}`));
    }, timeoutMs);
    child.stdout?.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr?.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", (err) => {
      clearTimeout(timer);
      reject(err);
    });
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      resolve({ exitCode: code ?? -1, stdout, stderr, signal });
    });
  });
}

async function runNpm(args: readonly string[], options: SpawnOptions, timeoutMs: number): Promise<void> {
  const npm = process.platform === "win32" ? "npm.cmd" : "npm";
  const result = await runCommand(npm, args, { ...options, timeoutMs });
  if (result.exitCode !== 0) {
    throw new Error(
      `npm ${args.join(" ")} exited with code ${result.exitCode}\n--- stdout ---\n${result.stdout}\n--- stderr ---\n${result.stderr}`
    );
  }
}
