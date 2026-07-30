import { spawn, type SpawnOptions } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export interface PublicPackageJson {
  readonly name: string;
  readonly version: string;
  readonly type?: string;
  readonly main?: string;
  readonly types?: string;
  readonly bin?: Record<string, string>;
  readonly exports?: unknown;
  readonly dependencies?: Record<string, string>;
}

export interface InstallResult {
  readonly installDir: string;
  readonly sdkDir: string;
  readonly cliDir: string;
  readonly sdkPackageJson: PublicPackageJson;
  readonly cliPackageJson: PublicPackageJson;
  readonly source: "tarballs" | "registry" | "local-pack";
  readonly cleanup: () => void;
}

export interface ArtifactSpecs {
  readonly sdk: string;
  readonly cli: string;
  readonly source: InstallResult["source"];
}

export interface CommandResult {
  readonly exitCode: number;
  readonly stdout: string;
  readonly stderr: string;
  readonly signal: NodeJS.Signals | null;
}

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "..", "..", "..", "..");
let localPackPromise: Promise<ArtifactSpecs> | undefined;

export async function resolveArtifactSpecs(
  env: NodeJS.ProcessEnv = process.env,
  packLocal: () => Promise<ArtifactSpecs> = packCurrentArtifacts
): Promise<ArtifactSpecs> {
  const sdkTarball = env.AEX_USER_TEST_SDK_TARBALL;
  const cliTarball = env.AEX_USER_TEST_CLI_TARBALL;
  const sdkVersion = env.AEX_USER_TEST_SDK_VERSION;
  const cliVersion = env.AEX_USER_TEST_CLI_VERSION;
  const anyTarball = Boolean(sdkTarball || cliTarball);
  const anyVersion = Boolean(sdkVersion || cliVersion);
  if (anyTarball && anyVersion) {
    throw new Error("user-tests: tarball and registry artifact selectors are mutually exclusive");
  }
  if (anyTarball) {
    if (!sdkTarball || !cliTarball) {
      throw new Error("user-tests: both AEX_USER_TEST_SDK_TARBALL and AEX_USER_TEST_CLI_TARBALL are required");
    }
    for (const path of [sdkTarball, cliTarball]) {
      if (!existsSync(path)) throw new Error(`user-tests: artifact tarball does not exist: ${path}`);
    }
    return { sdk: sdkTarball, cli: cliTarball, source: "tarballs" };
  }
  if (anyVersion) {
    if (!sdkVersion || !cliVersion) {
      throw new Error("user-tests: both AEX_USER_TEST_SDK_VERSION and AEX_USER_TEST_CLI_VERSION are required");
    }
    for (const version of [sdkVersion, cliVersion]) {
      if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
        throw new Error(`user-tests: artifact version must be exact semver, got ${version}`);
      }
    }
    return {
      sdk: `@aexhq/sdk@${sdkVersion}`,
      cli: `@aexhq/cli@${cliVersion}`,
      source: "registry"
    };
  }
  localPackPromise ??= packLocal();
  return await localPackPromise;
}

export async function installAex(): Promise<InstallResult> {
  const specs = await resolveArtifactSpecs();
  const parent = process.env.AEX_USER_TEST_TEMP_ROOT;
  const installDir = mkdtempSync(join(parent || tmpdir(), "aex-user-test-install-"));
  const cleanup = createDirectoryCleanup(installDir);
  writeFileSync(
    join(installDir, "package.json"),
    `${JSON.stringify({ name: "aex-user-test-host", private: true, type: "module" }, null, 2)}\n`
  );
  const installSpecs = specs.source === "registry"
    ? [specs.sdk, specs.cli]
    : [specs.sdk, specs.cli].map((source) => {
        const target = join(installDir, basename(source));
        copyFileSync(source, target);
        return target;
      });
  try {
    const result = await runCommand(
      getBunCommand(),
      ["install", ...installSpecs, "--ignore-scripts", "--no-progress"],
      { cwd: installDir, timeoutMs: 180_000 }
    );
    if (result.exitCode !== 0) {
      throw new Error(`bun install failed\n${result.stdout}\n${result.stderr}`);
    }
    const sdkDir = join(installDir, "node_modules", "@aexhq", "sdk");
    const cliDir = join(installDir, "node_modules", "@aexhq", "cli");
    const sdkPackageJson = readPackageJson(sdkDir);
    const cliPackageJson = readPackageJson(cliDir);
    return {
      installDir,
      sdkDir,
      cliDir,
      sdkPackageJson,
      cliPackageJson,
      source: specs.source,
      cleanup
    };
  } catch (error) {
    cleanup();
    throw error;
  }
}

export function createDirectoryCleanup(path: string): () => void {
  let complete = false;
  return () => {
    if (complete) return;
    rmSync(path, { recursive: true, force: true });
    if (existsSync(path)) throw new Error(`user-tests: failed to remove install directory ${path}`);
    complete = true;
  };
}

async function packCurrentArtifacts(): Promise<ArtifactSpecs> {
  const packRoot = mkdtempSync(join(tmpdir(), "aex-user-test-pack-"));
  const build = await runCommand(getBunCommand(), ["run", "build"], {
    cwd: repoRoot,
    timeoutMs: 300_000
  });
  if (build.exitCode !== 0) {
    rmSync(packRoot, { recursive: true, force: true });
    throw new Error(`workspace build failed before user-test pack\n${build.stdout}\n${build.stderr}`);
  }
  try {
    const sdk = await packPackage("sdk", /^aexhq-sdk-.*\.tgz$/, packRoot);
    const cli = await packPackage("cli", /^aexhq-cli-.*\.tgz$/, packRoot);
    return { sdk, cli, source: "local-pack" };
  } catch (error) {
    rmSync(packRoot, { recursive: true, force: true });
    throw error;
  }
}

async function packPackage(name: string, pattern: RegExp, packRoot: string): Promise<string> {
  const packageDir = join(repoRoot, "packages", name);
  const result = await runCommand(
    getBunCommand(),
    ["pm", "pack", "--destination", packRoot, "--ignore-scripts"],
    { cwd: packageDir, timeoutMs: 180_000 }
  );
  if (result.exitCode !== 0) throw new Error(`could not pack ${name}\n${result.stdout}\n${result.stderr}`);
  const matches = readdirSync(packRoot).filter((file) => pattern.test(file));
  if (matches.length !== 1) throw new Error(`expected one ${name} tarball, found ${matches.length}`);
  return join(packRoot, matches[0]!);
}

function readPackageJson(packageDir: string): PublicPackageJson {
  const path = join(packageDir, "package.json");
  if (!existsSync(path)) throw new Error(`installed package manifest is missing: ${path}`);
  return JSON.parse(readFileSync(path, "utf8")) as PublicPackageJson;
}

export async function runCommand(
  command: string,
  args: readonly string[],
  options: SpawnOptions & { readonly timeoutMs?: number } = {}
): Promise<CommandResult> {
  const timeoutMs = options.timeoutMs ?? 60_000;
  const { timeoutMs: _timeout, ...spawnOptions } = options;
  void _timeout;
  return await new Promise<CommandResult>((resolvePromise, reject) => {
    let stdout = "";
    let stderr = "";
    const child = spawn(command, [...args], {
      ...spawnOptions,
      shell: false,
      stdio: ["ignore", "pipe", "pipe"]
    });
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`command timed out after ${timeoutMs}ms: ${command} ${args.join(" ")}`));
    }, timeoutMs);
    child.stdout?.on("data", (chunk) => { stdout += chunk.toString(); });
    child.stderr?.on("data", (chunk) => { stderr += chunk.toString(); });
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", (code, signal) => {
      clearTimeout(timer);
      resolvePromise({ exitCode: code ?? -1, stdout, stderr, signal });
    });
  });
}

export function getBunCommand(): string {
  if (process.env.AEX_USER_TEST_BUN) return process.env.AEX_USER_TEST_BUN;
  if (process.env.BUN) return process.env.BUN;
  if ("bun" in process.versions) return process.execPath;
  const installed = process.env.BUN_INSTALL
    ? join(process.env.BUN_INSTALL, "bin", process.platform === "win32" ? "bun.exe" : "bun")
    : join(homedir(), ".bun", "bin", process.platform === "win32" ? "bun.exe" : "bun");
  return existsSync(installed) ? installed : process.platform === "win32" ? "bun.exe" : "bun";
}

export function getAexBinPath(installDir: string): string {
  const binDir = join(installDir, "node_modules", ".bin");
  const candidates = process.platform === "win32" ? ["aex.exe", "aex.cmd", "aex"] : ["aex"];
  return candidates.map((name) => join(binDir, name)).find(existsSync) ?? join(binDir, candidates[0]!);
}
