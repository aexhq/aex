/**
 * Shared install fixture for the user-tests layer.
 *
 * Each scenario calls {@link installAex} which:
 *   1. Reads AEX_USER_TEST_TARBALL or AEX_USER_TEST_VERSION
 *      (explicit overrides stay mutually exclusive). When neither is
 *      set, local/offline sessions pack the current workspace SDK once and
 *      install that tarball.
 *   2. Creates a fresh tempdir.
 *   3. Materializes a minimal package.json there.
 *   4. Sessions `bun install <tarball|@aexhq/sdk@version>` against it.
 *   5. Returns paths for the install so scenarios can spawn child
 *      processes with cwd = installDir.
 *
 * Cleanup is the test's responsibility (typically in afterAll).
 *
 * Install reuse: `installAex()` returns a per-worker SHARED install by
 * default. The tree is read-only after Bun install — every read-only
 * scenario only drops UNIQUELY-named sibling scripts and reads
 * node_modules — so all such scenarios in a worker reuse ONE install
 * instead of each paying a redundant `bun install`. Scenarios that
 * MUTATE the tree (install extra packages, write fixed-name sources —
 * e.g. the TS consumer scenario) must pass `{ isolated: true }` to get
 * their own clean tree. The shared tree is removed once at process exit;
 * isolated trees are the caller's responsibility (typically afterAll).
 */
import { spawn, type SpawnOptions } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export interface InstallResult {
  /** Absolute path to the install tempdir (Bun install was run here). */
  readonly installDir: string;
  /** `node_modules/@aexhq/sdk` inside the install tempdir. */
  readonly aexDir: string;
  /** `node_modules/@aexhq/sdk/package.json` parsed. */
  readonly aexPackageJson: AexPackageJson;
  /** Resolved version of aex that was installed. */
  readonly resolvedVersion: string;
  /** Spec passed to Bun install (tarball path or `@aexhq/sdk@<version>`). */
  readonly installSpec: string;
  /** Source kind for diagnostics. */
  readonly source: "tarball" | "registry" | "local-pack";
  /** SessionRecord cleanup: removes installDir recursively. Safe to call twice. */
  readonly cleanup: () => void;
}

export interface AexPackageJson {
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
  /**
   * Force a fresh, isolated install tree instead of the per-worker
   * shared one. Required for scenarios that MUTATE the tree (install
   * extra packages, write fixed-name sources), e.g. the typescript
   * consumer test. Read-only scenarios should omit this and share.
   */
  readonly isolated?: boolean;
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
  `aex-user-test-sdk-pack-${createHash("sha256").update(repoRoot).digest("hex").slice(0, 16)}.lock`
);
const generatedDistLockScript = join(repoRoot, "scripts", "with-generated-dist-lock.mjs");
let localSdkPackPromise: Promise<string> | null = null;

/**
 * Resolve which artifact the runner is testing.
 * Exported separately so the diagnostic header can print it before
 * actually running Bun install.
 */
export async function resolveInstallSpec(
  options: ResolveInstallSpecOptions = {}
): Promise<{ readonly spec: string; readonly source: InstallResult["source"] }> {
  const env = options.env ?? process.env;
  const pathExists = options.pathExists ?? existsSync;
  const packLocalSdk = options.packLocalSdk ?? packCurrentSdkOnce;
  const tarball = env["AEX_USER_TEST_TARBALL"];
  const version = env["AEX_USER_TEST_VERSION"];
  if (tarball && version) {
    throw new Error(
      "user-tests: AEX_USER_TEST_TARBALL and AEX_USER_TEST_VERSION are mutually exclusive. Set exactly one."
    );
  }
  if (tarball) {
    if (!pathExists(tarball)) {
      throw new Error(`user-tests: AEX_USER_TEST_TARBALL points at a non-existent path: ${tarball}`);
    }
    return { spec: tarball, source: "tarball" };
  }
  if (version) {
    if (!/^[0-9]+\.[0-9]+\.[0-9]+([\-+][0-9A-Za-z.\-]+)?$/.test(version)) {
      throw new Error(
        `user-tests: AEX_USER_TEST_VERSION must be a concrete semver (e.g. 0.2.1), got: ${version}`
      );
    }
    return { spec: `@aexhq/sdk@${version}`, source: "registry" };
  }
  const localTarball = await packLocalSdk();
  return { spec: localTarball, source: "local-pack" };
}

/**
 * Install the resolved aex artifact. Returns a per-worker SHARED install
 * by default (memoized for the worker process — see file header); pass
 * `{ isolated: true }` for scenarios that mutate the tree.
 */
export async function installAex(options: InstallOptions = {}): Promise<InstallResult> {
  if (options.isolated) {
    return await installAexIsolated(options);
  }
  return await getSharedInstall(options);
}

let sharedInstallPromise: Promise<InstallResult> | null = null;
const deferredSharedCleanups: Array<() => void> = [];
let sharedExitHookRegistered = false;

/**
 * Memoized per worker PROCESS. vitest distributes test files across worker
 * processes, so each worker memoizes its OWN shared dir — no cross-process
 * FS race. The wrapped cleanup() is a no-op because per-file afterAll hooks
 * must NOT delete a tree later files in the same worker still reuse; the
 * real dir is removed once at process exit. Options after the first call
 * are ignored (the first caller wins).
 */
function getSharedInstall(options: InstallOptions): Promise<InstallResult> {
  sharedInstallPromise ??= installAexIsolated(options).then((result) => {
    registerSharedExitCleanup();
    deferredSharedCleanups.push(result.cleanup);
    return { ...result, cleanup: () => {} };
  });
  return sharedInstallPromise;
}

function registerSharedExitCleanup(): void {
  if (sharedExitHookRegistered) return;
  sharedExitHookRegistered = true;
  process.once("exit", () => {
    for (const cleanup of deferredSharedCleanups) {
      try {
        cleanup();
      } catch {
        // Best effort at process exit.
      }
    }
  });
}

/**
 * Install the resolved aex artifact into a fresh tempdir.
 * Throws if Bun exits non-zero or installs the wrong version.
 */
async function installAexIsolated(options: InstallOptions = {}): Promise<InstallResult> {
  const { spec, source } = await resolveInstallSpec();
  const installDir = mkdtempSync(join(tmpdir(), "aex-user-test-"));
  // Minimal host package.json so Bun install has a host project.
  const hostPkg = {
    name: "aex-user-test-host",
    version: "0.0.0",
    private: true,
    type: "module"
  };
  writeFileSync(join(installDir, "package.json"), JSON.stringify(hostPkg, null, 2));

  const installSpec =
    source === "registry"
      ? spec
      : (() => {
          const localSpec = join(installDir, basename(spec));
          copyFileSync(spec, localSpec);
          return localSpec;
        })();
  const args = ["install", installSpec, "--ignore-scripts", "--no-progress"];
  if (options.registryUrl) {
    args.push("--registry", options.registryUrl);
  }

  const timeoutMs = options.timeoutMs ?? 120_000;
  try {
    await runBun(args, { cwd: installDir }, timeoutMs);
  } catch (error) {
    rmSync(installDir, { recursive: true, force: true });
    throw error;
  }

  const aexDir = join(installDir, "node_modules", "@aexhq", "sdk");
  if (!existsSync(aexDir)) {
    rmSync(installDir, { recursive: true, force: true });
    throw new Error(`user-tests: install completed but ${aexDir} is missing`);
  }
  const pkgPath = join(aexDir, "package.json");
  if (!existsSync(pkgPath)) {
    rmSync(installDir, { recursive: true, force: true });
    throw new Error(`user-tests: install completed but ${pkgPath} is missing`);
  }
  let pkg: AexPackageJson;
  try {
    const text = await import("node:fs/promises").then((m) => m.readFile(pkgPath, "utf8"));
    pkg = JSON.parse(text) as AexPackageJson;
  } catch (error) {
    rmSync(installDir, { recursive: true, force: true });
    throw new Error(`user-tests: could not read installed package.json: ${(error as Error).message}`);
  }

  if (source === "registry") {
    const want = spec.replace(/^@aexhq\/sdk@/, "");
    if (pkg.version !== want) {
      rmSync(installDir, { recursive: true, force: true });
      throw new Error(
        `user-tests: registry resolved @aexhq/sdk@${want} but installed package reports version ${pkg.version}`
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
    aexDir,
    aexPackageJson: pkg,
    resolvedVersion: pkg.version,
    installSpec,
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
    const packDir = mkdtempSync(join(tmpdir(), "aex-user-test-sdk-pack-"));
    try {
      await runCommand(getBunCommand(), [generatedDistLockScript, "bun", "pm", "pack", "--destination", packDir], {
        cwd: join(repoRoot, "packages", "sdk"),
        timeoutMs: 180_000
      }).then((result) => {
        if (result.exitCode !== 0) {
          throw new Error(
            `bun pm pack --destination ${packDir} exited with code ${result.exitCode}\n--- stdout ---\n${result.stdout}\n--- stderr ---\n${result.stderr}`
          );
        }
      });
      const tarballs = readdirSync(packDir).filter((name) => /^aexhq-sdk-.*\.tgz$/.test(name));
      if (tarballs.length !== 1) {
        throw new Error(`user-tests: expected one packed @aexhq/sdk tarball in ${packDir}, found ${tarballs.length}`);
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
 * SessionRecord a shell command. Returns { stdout, stderr, exitCode }.
 * Rejects on timeout or spawn error; never rejects on non-zero exit
 * — the caller decides whether non-zero is a failure.
 */
export interface SessionResult {
  readonly exitCode: number;
  readonly stdout: string;
  readonly stderr: string;
  readonly signal: NodeJS.Signals | null;
}

interface PreparedSpawn {
  readonly command: string;
  readonly args: string[];
  readonly options: Pick<SpawnOptions, "shell" | "windowsVerbatimArguments">;
}

function prepareSpawn(command: string, args: readonly string[], env: NodeJS.ProcessEnv): PreparedSpawn {
  if (isWindowsCommandShim(command)) {
    return {
      command: env.ComSpec ?? "cmd.exe",
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

function isWindowsCommandShim(command: string): boolean {
  return process.platform === "win32" && /\.(?:cmd|bat)$/i.test(command);
}

function windowsCommandLine(command: string, args: readonly string[]): string {
  const argv = [command, ...args].map(quoteWindowsCommandArg).join(" ");
  return `"${argv}"`;
}

function quoteWindowsCommandArg(value: string): string {
  if (value.length === 0) return '""';
  return `"${value.replace(/(\\*)"/g, '$1$1\\"').replace(/(\\+)$/g, "$1$1")}"`;
}

export async function runCommand(
  command: string,
  args: readonly string[],
  options: SpawnOptions & { readonly timeoutMs?: number } = {}
): Promise<SessionResult> {
  const timeoutMs = options.timeoutMs ?? 60_000;
  const { timeoutMs: _omit, ...spawnOptions } = options;
  void _omit;
  const env = withBunOnPath(spawnOptions.env);
  const prepared = prepareSpawn(command, args, env);
  return await new Promise<SessionResult>((resolve, reject) => {
    let stdout = "";
    let stderr = "";
    const child = spawn(prepared.command, prepared.args, {
      ...spawnOptions,
      env,
      stdio: ["ignore", "pipe", "pipe"],
      ...prepared.options
    });
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(
        new Error(
          `timed out after ${timeoutMs}ms: ${command} ${args.join(" ")}\n` +
            `--- stdout ---\n${stdout}\n--- stderr ---\n${stderr}`
        )
      );
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

export function getBunCommand(): string {
  if (process.env.AEX_USER_TEST_BUN) return process.env.AEX_USER_TEST_BUN;
  if (process.env.BUN) return process.env.BUN;
  if ("bun" in process.versions) return process.execPath;

  const bunInstall = process.env.BUN_INSTALL;
  const candidates = [
    bunInstall ? join(bunInstall, "bin", process.platform === "win32" ? "bun.exe" : "bun") : undefined,
    join(homedir(), ".bun", "bin", process.platform === "win32" ? "bun.exe" : "bun")
  ];
  for (const candidate of candidates) {
    if (candidate && existsSync(candidate)) return candidate;
  }

  return process.platform === "win32" ? "bun.exe" : "bun";
}

function withBunOnPath(env: NodeJS.ProcessEnv | undefined): NodeJS.ProcessEnv {
  const next = { ...process.env, ...env };
  const pathKey = Object.keys(next).find((key) => key.toLowerCase() === "path") ?? "PATH";
  const bunDir = dirname(getBunCommand());
  const currentPath = next[pathKey] ?? "";
  next[pathKey] = currentPath.length > 0 ? `${bunDir}${process.platform === "win32" ? ";" : ":"}${currentPath}` : bunDir;
  return next;
}

export function getAexBinPath(installDir: string): string {
  const binDir = join(installDir, "node_modules", ".bin");
  const candidates = process.platform === "win32" ? ["aex.exe", "aex.cmd", "aex"] : ["aex"];
  for (const candidate of candidates) {
    const path = join(binDir, candidate);
    if (existsSync(path)) return path;
  }
  return join(binDir, candidates[0]!);
}

async function runBun(args: readonly string[], options: SpawnOptions, timeoutMs: number): Promise<void> {
  const result = await runCommand(getBunCommand(), args, { ...options, timeoutMs });
  if (result.exitCode !== 0) {
    throw new Error(
      `bun ${args.join(" ")} exited with code ${result.exitCode}\n--- stdout ---\n${result.stdout}\n--- stderr ---\n${result.stderr}`
    );
  }
}
