import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { availableParallelism, tmpdir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  assertExecutionAllowed,
  buildAffectedPlan,
  mergeChangedPaths,
  parseArguments,
  parseNullSeparated,
  type CommandPlan,
  type NodeDependencyGraph,
  type SelectionDocument
} from "./affected.js";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

try {
  const options = parseArguments(
    process.argv.slice(2),
    Math.max(1, Math.min(8, availableParallelism()))
  );
  if (options.help) {
    printHelp();
    process.exit(0);
  }

  const changedPaths = collectChangedPaths(options.base);
  if (changedPaths.length === 0) {
    console.log(`aex affected: no changes relative to ${options.base}; nothing to run`);
    process.exit(0);
  }

  const targetDir = options.targetDir === undefined
    ? undefined
    : isAbsolute(options.targetDir)
      ? options.targetDir
      : resolve(repoRoot, options.targetDir);
  const environment = targetDir === undefined
    ? process.env
    : { ...process.env, CARGO_TARGET_DIR: targetDir };
  const selection = select(changedPaths, options.releaseTool, environment);
  const plan = buildAffectedPlan(
    selection,
    options.actions,
    options.jobs,
    loadNodeDependencyGraph()
  );
  const branch = gitText(["branch", "--show-current"]);

  printPlan(plan, targetDir);
  if (options.actions.length === 0) process.exit(0);
  assertExecutionAllowed(plan, branch);
  for (const command of plan.commands) run(command, environment);
} catch (error) {
  console.error(`aex affected: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}

function collectChangedPaths(base: string): string[] {
  const mergeBase = gitText(["merge-base", base, "HEAD"]);
  return mergeChangedPaths(
    gitPaths(["diff", "--no-renames", "--name-only", "-z", `${mergeBase}..HEAD`, "--"]),
    gitPaths(["diff", "--no-renames", "--name-only", "-z", "HEAD", "--"]),
    gitPaths(["ls-files", "--others", "--exclude-standard", "-z"])
  );
}

function gitPaths(args: readonly string[]): string[] {
  return parseNullSeparated(git(args));
}

function gitText(args: readonly string[]): string {
  return git(args).trim();
}

function git(args: readonly string[]): string {
  return execFileSync("git", ["-C", repoRoot, ...args], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"]
  });
}

interface NodeManifest {
  readonly name?: unknown;
  readonly dependencies?: unknown;
  readonly devDependencies?: unknown;
  readonly peerDependencies?: unknown;
}

function loadNodeDependencyGraph(): NodeDependencyGraph {
  const manifests = new Map<string, NodeManifest>();
  const paths = gitPaths([
    "ls-files",
    "--cached",
    "--others",
    "--exclude-standard",
    "-z",
    "--",
    "*package.json"
  ]);

  for (const path of paths) {
    const manifestPath = join(repoRoot, path);
    if (!existsSync(manifestPath)) continue;
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as NodeManifest;
    if (typeof manifest.name !== "string") continue;
    if (manifests.has(manifest.name)) {
      throw new Error(`duplicate npm workspace package name ${manifest.name}`);
    }
    manifests.set(manifest.name, manifest);
  }

  const names = new Set(manifests.keys());
  const graph: Record<string, string[]> = {};
  for (const [name, manifest] of manifests) {
    const dependencies = [
      ...dependencyNames(manifest.dependencies),
      ...dependencyNames(manifest.devDependencies),
      ...dependencyNames(manifest.peerDependencies)
    ];
    graph[name] = [...new Set(dependencies)]
      .filter((dependency) => names.has(dependency))
      .sort();
  }
  return graph;
}

function dependencyNames(value: unknown): string[] {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return [];
  return Object.keys(value);
}

function select(
  changedPaths: readonly string[],
  releaseTool: string | undefined,
  environment: NodeJS.ProcessEnv
): SelectionDocument {
  const scratch = mkdtempSync(join(tmpdir(), "aex-dev-affected-"));
  const pathsFile = join(scratch, "paths.zlist");
  const selectionFile = join(scratch, "selection.json");
  try {
    writeFileSync(pathsFile, `${changedPaths.join("\0")}\0`);
    const args = [
      "--root",
      repoRoot,
      "graph",
      "select",
      "--paths-file",
      pathsFile,
      "--mode",
      "affected",
      "--lane",
      "pr",
      "--out",
      selectionFile,
      "--quiet"
    ];
    const command = releaseTool === undefined
      ? { program: "cargo", args: ["run", "--quiet", "--locked", "-p", "aex-release-tool", "--", ...args] }
      : { program: releaseTool, args };
    run(command, environment);
    return JSON.parse(readFileSync(selectionFile, "utf8")) as SelectionDocument;
  } finally {
    rmSync(scratch, { force: true, recursive: true });
  }
}

function printPlan(
  plan: ReturnType<typeof buildAffectedPlan>,
  targetDir: string | undefined
): void {
  console.log(
    `aex affected: ${plan.changedPaths} path(s), ${plan.rustPackages.length} Cargo package(s), ` +
      `${plan.nodePackages.length} npm package(s), ${plan.scenarios} scenario(s)`
  );
  console.log(`scope: ${plan.repoWide ? "full (release graph widened)" : "affected reverse closure"}`);
  if (targetDir !== undefined) console.log(`explicit CARGO_TARGET_DIR=${targetDir}`);
  if (plan.commands.length === 0) console.log("plan only; pass --run <action[,action]> to execute");
  for (const command of plan.commands) console.log(`$ ${render(command)}`);
  console.log(`not covered by this feedback command: ${plan.excludedEvidence.join("; ")}`);
}

function render(command: CommandPlan): string {
  return [command.program, ...command.args]
    .map((part) => (/^[A-Za-z0-9_@./:\\=-]+$/.test(part) ? part : JSON.stringify(part)))
    .join(" ");
}

function run(command: CommandPlan, environment: NodeJS.ProcessEnv): void {
  const result = spawnSync(command.program, command.args, {
    cwd: repoRoot,
    env: environment,
    stdio: "inherit"
  });
  if (result.error !== undefined) throw result.error;
  if (result.signal !== null) throw new Error(`${command.program} terminated by ${result.signal}`);
  if (result.status !== 0) {
    throw new Error(`${render(command)} exited ${result.status ?? "without a status"}`);
  }
}

function printHelp(): void {
  console.log(`Usage: bun run dev:affected -- [options]

Plan the exact Cargo and npm reverse closure selected by the release graph.
Planning is the default; execution is explicit.

  --run <actions>       Comma-separated fmt,build,check,clippy,test; or all
  --base <git-ref>      Compare the merge base of this ref with HEAD (default: main)
  --jobs <count>        Cap Cargo build and nextest concurrency (default: min(CPUs, 8))
  --target-dir <path>   Explicitly opt into another Cargo target cache
  --release-tool <path> Use an already-built aex-release-tool binary
  -h, --help            Show this help

Committed, staged, unstaged, deleted, and untracked paths are combined. Cargo
lanes run sequentially because parallel Cargo processes contend for the same
target lock; Cargo/nextest parallelize internally, and affected npm packages
run in parallel. This is fast local feedback, never CI or release evidence.`);
}
