#!/usr/bin/env node

/**
 * The four module lanes, run for one module: build, typecheck, lint, unit tests.
 *
 * The module boundary is the unit of dependency analysis, so the lanes are per
 * module rather than per repository. Two properties matter more than convenience:
 *
 *  - A lane a module does not declare is REPORTED, not silently absent. `apps/docs`
 *    has no `test:unit`; that must read as "this module declares no unit lane",
 *    never as "unit tests passed".
 *  - Every declared lane runs to completion even after an earlier one fails, then
 *    the script exits non-zero. A module whose typecheck fails still tells you
 *    whether its tests fail too, which is the difference between one fix and four
 *    round trips.
 */

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { readModuleGraph } from "./public-module-graph.mjs";

/** Ordered because a later lane consumes the earlier lane's output. */
export const MODULE_LANES = ["build", "typecheck", "lint", "test:unit"];

export function planModuleLanes(repoRoot, moduleId) {
  const graph = readModuleGraph(repoRoot);
  const node = graph.byId.get(moduleId);
  if (!node) throw new Error(`unknown public module: ${moduleId}`);
  const manifest = JSON.parse(readFileSync(resolve(repoRoot, node.dir, "package.json"), "utf8"));
  const scripts = manifest.scripts ?? {};
  return {
    module: node.id,
    name: node.name,
    declared: MODULE_LANES.filter((lane) => typeof scripts[lane] === "string"),
    undeclared: MODULE_LANES.filter((lane) => typeof scripts[lane] !== "string")
  };
}

export function runModuleLanes(repoRoot, moduleId, run = defaultRun) {
  const plan = planModuleLanes(repoRoot, moduleId);
  if (plan.declared.length === 0) {
    throw new Error(`module ${moduleId} declares none of: ${MODULE_LANES.join(", ")}`);
  }
  const results = [];
  for (const lane of plan.declared) {
    const status = run(repoRoot, plan.name, lane);
    results.push({ lane, status });
  }
  return { ...plan, results, ok: results.every((entry) => entry.status === 0) };
}

function defaultRun(repoRoot, packageName, lane) {
  const result = spawnSync("bun", ["run", "--filter", packageName, lane], {
    cwd: repoRoot,
    stdio: "inherit",
    env: process.env,
    shell: process.platform === "win32"
  });
  if (result.error) throw result.error;
  return result.status ?? 1;
}

export function main(argv = process.argv.slice(2)) {
  const moduleId = argv[0];
  if (!moduleId || moduleId.startsWith("--")) throw new Error("usage: run-module-checks.mjs <module-id>");
  const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
  const outcome = runModuleLanes(repoRoot, moduleId);
  for (const lane of outcome.undeclared) {
    process.stdout.write(`module-lane ${outcome.module} ${lane}: NOT DECLARED by ${outcome.name}\n`);
  }
  for (const entry of outcome.results) {
    process.stdout.write(
      `module-lane ${outcome.module} ${entry.lane}: ${entry.status === 0 ? "pass" : `fail (exit ${entry.status})`}\n`
    );
  }
  if (!outcome.ok) process.exitCode = 1;
  return outcome;
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`::error::${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  }
}
