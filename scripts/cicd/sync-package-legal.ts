/**
 * Propagates the repository LICENSE and NOTICE into every publishable package.
 *
 * WHY the copies exist at all. Apache 2.0 §4(a) obliges a redistributor to give
 * recipients a copy of the licence, and §4(d) makes NOTICE mandatory to
 * propagate once it exists. An npm tarball is a redistribution, and a tarball
 * cannot reach up to a repository root that is not inside it. Before this
 * script, `bun pm pack` on `packages/sdk` produced a tarball with no licence
 * text at all — the SPDX string in `package.json` is metadata, not the licence.
 *
 * WHY they are generated rather than hand-written. Three copies now, 28 after
 * the engine extraction. A hand-edited copy drifts silently, and a drifted
 * licence is the one kind of drift that is a legal problem rather than a
 * tidiness problem. Root is the single source, this script is the propagation,
 * and `--check` is the gate that fails when a copy stops matching.
 *
 * Usage:
 *   bun scripts/cicd/sync-package-legal.ts           # write
 *   bun scripts/cicd/sync-package-legal.ts --check   # fail on drift
 */
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { REPO_ROOT, isFile, listPublishableModules } from "./public-modules.js";

/** The files copied into each publishable package, in the order reported. */
export const LEGAL_FILES = ["LICENSE", "NOTICE"] as const;

function readRoot(repoRoot: string, name: string): string {
  return readFileSync(join(repoRoot, name), "utf8");
}

/**
 * @returns the packages whose copy is missing or differs, as
 *   `"<directory>/<file>"` strings, sorted.
 */
export function findDrift(repoRoot: string = REPO_ROOT): string[] {
  const drift: string[] = [];
  for (const module of listPublishableModules(repoRoot)) {
    for (const name of LEGAL_FILES) {
      const target = join(module.dir, name);
      if (!isFile(target) || readFileSync(target, "utf8") !== readRoot(repoRoot, name)) {
        drift.push(`${module.relativeDir}/${name}`);
      }
    }
  }
  return drift.sort();
}

/** Writes the root copies into every publishable package. @returns how many. */
export function syncLegalFiles(repoRoot: string = REPO_ROOT): number {
  const modules = listPublishableModules(repoRoot);
  for (const module of modules) {
    for (const name of LEGAL_FILES) {
      writeFileSync(join(module.dir, name), readRoot(repoRoot, name), "utf8");
    }
  }
  return modules.length;
}

function main(): number {
  if (process.argv.includes("--check")) {
    const drift = findDrift();
    if (drift.length === 0) {
      process.stdout.write(
        "package-legal OK: every publishable package carries the root LICENSE and NOTICE.\n"
      );
      return 0;
    }
    process.stderr.write(
      `package-legal: ${drift.length} file(s) missing or out of date:\n` +
        drift.map((entry) => `  ${entry}\n`).join("") +
        "Run: bun scripts/cicd/sync-package-legal.ts\n"
    );
    return 1;
  }

  const count = syncLegalFiles();
  process.stdout.write(`package-legal: synced ${LEGAL_FILES.join(", ")} into ${count} package(s).\n`);
  return 0;
}

function isEntryPoint(): boolean {
  const entry = process.argv[1];
  return entry !== undefined && resolve(entry) === resolve(fileURLToPath(import.meta.url));
}

if (isEntryPoint()) process.exit(main());
