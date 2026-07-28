/**
 * The public module registry.
 *
 * WHY this exists. The service/module release model makes every public module an
 * independently addressable npm package with its own canary. Three separate
 * consumers need the same answer to "which packages are those?":
 *
 *   1. `sync-package-legal.ts` — which directories need LICENSE + NOTICE.
 *   2. `scripts/validate/package-metadata.test.ts` — which manifests are held to
 *      the publishable-metadata contract.
 *   3. the per-module canary publication lane — which packages get a matrix
 *      entry.
 *
 * A hand-maintained list would be a fourth place to forget a package, and the
 * engine extraction adds 25 of them in one commit. So the registry is DERIVED: a
 * workspace member is a public module exactly when its manifest does not say
 * `private: true`. That is already the field npm itself obeys, so the registry
 * and the publisher cannot disagree.
 *
 * Deliberately NOT derived: the workspace globs. They come from the root
 * manifest, which is the same source `bun install` reads.
 *
 * Usage:
 *   bun scripts/cicd/public-modules.ts           # name, version, directory
 *   bun scripts/cicd/public-modules.ts --json
 */
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/** The manifest fields this module reads. Deliberately not the whole schema. */
export interface PackageManifest {
  readonly name?: string;
  readonly version?: string;
  readonly private?: boolean;
  readonly workspaces?: readonly string[];
  readonly dependencies?: Readonly<Record<string, string>>;
  readonly [key: string]: unknown;
}

export interface WorkspaceModule {
  /** Absolute package directory. */
  readonly dir: string;
  /** Repo-relative directory with forward slashes — the `repository.directory` form. */
  readonly relativeDir: string;
  readonly manifestPath: string;
  readonly manifest: PackageManifest;
}

/** Directories never scanned for manifests, at any depth. */
const SKIP_DIRECTORIES = new Set([
  ".git",
  "node_modules",
  "dist",
  ".next",
  ".turbo",
  ".source",
  ".generated",
  ".tmp",
  "coverage"
]);

function toPosix(value: string): string {
  return value.split("\\").join("/");
}

function readManifest(path: string): PackageManifest {
  return JSON.parse(readFileSync(path, "utf8")) as PackageManifest;
}

/**
 * Resolves the root `workspaces` globs to directories. Supports the two forms
 * this repository uses today and the two the extraction adds: a `dir/*` fan-out
 * and a literal path such as `runner-image`.
 */
export function workspaceDirectories(repoRoot: string = REPO_ROOT): string[] {
  const patterns = readManifest(join(repoRoot, "package.json")).workspaces ?? [];
  const out: string[] = [];
  for (const pattern of patterns) {
    if (pattern.endsWith("/*")) {
      const parent = join(repoRoot, pattern.slice(0, -2));
      if (!existsSync(parent)) continue;
      for (const entry of readdirSync(parent, { withFileTypes: true })) {
        if (!entry.isDirectory()) continue;
        const dir = join(parent, entry.name);
        if (existsSync(join(dir, "package.json"))) out.push(dir);
      }
      continue;
    }
    const dir = join(repoRoot, pattern);
    if (existsSync(join(dir, "package.json"))) out.push(dir);
  }
  return out.sort();
}

/**
 * Every `package.json` tracked in the repository, workspace member or not. The
 * licence-declaration rule applies to all of them; the publishable-metadata
 * contract does not.
 */
export function allPackageManifests(repoRoot: string = REPO_ROOT): string[] {
  const out: string[] = [];
  walk(repoRoot);
  return out.sort();

  function walk(dir: string): void {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        if (SKIP_DIRECTORIES.has(entry.name)) continue;
        walk(join(dir, entry.name));
        continue;
      }
      if (entry.name === "package.json") out.push(join(dir, "package.json"));
    }
  }
}

/** Every workspace member, in path order. */
export function listWorkspaceModules(repoRoot: string = REPO_ROOT): WorkspaceModule[] {
  return workspaceDirectories(repoRoot).map((dir) => ({
    dir,
    relativeDir: toPosix(relative(repoRoot, dir)),
    manifestPath: join(dir, "package.json"),
    manifest: readManifest(join(dir, "package.json"))
  }));
}

/**
 * The public modules: workspace members npm would publish.
 *
 * `private: true` is the single switch. A package that must never reach the
 * registry sets it; everything else is a public module and is held to the full
 * metadata contract.
 */
export function listPublishableModules(repoRoot: string = REPO_ROOT): WorkspaceModule[] {
  return listWorkspaceModules(repoRoot).filter((module) => module.manifest.private !== true);
}

/** Production dependency names of a manifest, excluding first-party packages. */
export function thirdPartyRuntimeDependencies(manifest: PackageManifest): string[] {
  return Object.keys(manifest.dependencies ?? {})
    .filter((name) => !name.startsWith("@aexhq/"))
    .sort();
}

/** Whether `path` is an existing file. Shared by the legal sync and its gate. */
export function isFile(path: string): boolean {
  return existsSync(path) && statSync(path).isFile();
}

/**
 * Entry detection by argv rather than `import.meta.main`, which is not present
 * on every runtime that executes these scripts.
 */
function isEntryPoint(): boolean {
  const entry = process.argv[1];
  return entry !== undefined && resolve(entry) === resolve(fileURLToPath(import.meta.url));
}

if (isEntryPoint()) {
  const modules = listPublishableModules();
  if (process.argv.includes("--json")) {
    process.stdout.write(
      `${JSON.stringify(
        modules.map((module) => ({
          name: module.manifest.name,
          version: module.manifest.version,
          directory: module.relativeDir
        })),
        null,
        2
      )}\n`
    );
  } else {
    for (const module of modules) {
      process.stdout.write(
        `${module.manifest.name}\t${module.manifest.version}\t${module.relativeDir}\n`
      );
    }
  }
}
