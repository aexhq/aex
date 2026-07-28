#!/usr/bin/env node

/**
 * The public repository's module graph and change router.
 *
 * Every public module is an independently addressable npm package, so a public
 * change must run the changed module AND its dependent closure, then publish an
 * individual canary for each affected publishable module. This script is the one
 * place that answers "which modules, and in what order" — the graph is DERIVED
 * from the workspace manifests rather than restated in a second registry file,
 * so a new package or a new dependency edge cannot drift out of the router.
 *
 * Failure posture, deliberately asymmetric:
 *
 *   verification  fails OPEN  — a router that cannot decide runs everything.
 *   publication   fails CLOSED — a router that cannot decide publishes nothing.
 *
 * Both are the safe direction for their operation, and both are expressed as
 * OUTPUTS rather than as an exit code: the caller's `if:` guards read
 * `routing_failed`, so `writeOutputs` must run on the error path too. A detector
 * that skips its outputs on failure routes every downstream lane to "run
 * nothing", which reads exactly like "everything passed".
 *
 * Graph correctness is therefore NOT gated by this script's exit code. It is
 * gated by `scripts/validate/public-module-graph.test.ts`, which runs in a
 * blocking lane and fails the build on a malformed graph.
 */

import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const WORKSPACE_PROTOCOL = /^workspace:/;
const FILE_PROTOCOL = /^file:\.\/(.+)$/;
const SHA = /^[0-9a-f]{7,40}$/;

/**
 * Read every workspace package as a module node.
 *
 * @param {string} repoRoot
 * @returns {{ modules: readonly object[], byId: Map<string, object>, byName: Map<string, object> }}
 */
export function readModuleGraph(repoRoot) {
  const rootManifest = JSON.parse(readFileSync(resolve(repoRoot, "package.json"), "utf8"));
  const globs = rootManifest.workspaces;
  if (!Array.isArray(globs) || globs.length === 0) {
    throw new Error("root package.json must declare a non-empty workspaces array");
  }

  const directories = [];
  for (const glob of globs) {
    const match = /^([A-Za-z0-9._-]+)\/\*$/.exec(String(glob));
    if (!match) {
      // Fail closed: an unsupported glob shape would silently drop modules from
      // the router while the workspace still contains them.
      throw new Error(`unsupported workspaces glob: ${glob}`);
    }
    const parent = resolve(repoRoot, match[1]);
    for (const entry of readdirSync(parent).sort()) {
      const dir = resolve(parent, entry);
      if (!statSync(dir).isDirectory()) continue;
      let manifest;
      try {
        manifest = readFileSync(resolve(dir, "package.json"), "utf8");
      } catch {
        continue;
      }
      directories.push({ id: entry, dir: `${match[1]}/${entry}`, manifest: JSON.parse(manifest) });
    }
  }
  if (directories.length === 0) throw new Error("the workspace globs matched no packages");

  // The root `overrides` map is how a bare specifier such as `aex` resolves to a
  // workspace directory. Without it `apps/docs` looks like it depends on an
  // external package and the SDK loses a dependent.
  const overrides = new Map();
  for (const [specifier, target] of Object.entries(rootManifest.overrides ?? {})) {
    const filePath = FILE_PROTOCOL.exec(String(target));
    if (filePath) overrides.set(specifier, filePath[1].replace(/\\/g, "/"));
  }

  const byName = new Map();
  const byId = new Map();
  for (const entry of directories) {
    const name = entry.manifest.name;
    if (typeof name !== "string" || name.length === 0) {
      throw new Error(`${entry.dir}/package.json has no name`);
    }
    if (byName.has(name)) throw new Error(`duplicate workspace package name: ${name}`);
    if (byId.has(entry.id)) throw new Error(`duplicate workspace directory name: ${entry.id}`);
    const node = {
      id: entry.id,
      name,
      dir: entry.dir,
      version: String(entry.manifest.version ?? ""),
      publishable: entry.manifest.private !== true,
      dependsOn: []
    };
    byName.set(name, node);
    byId.set(entry.id, node);
  }

  for (const entry of directories) {
    const node = byId.get(entry.id);
    const specifiers = new Set();
    for (const field of ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"]) {
      for (const [specifier, range] of Object.entries(entry.manifest[field] ?? {})) {
        // devDependencies count on purpose: `@aexhq/sdk` bundles `@aexhq/contracts`
        // at build time through a devDependency, so a contracts change changes the
        // published SDK artifact even though it is absent from runtime deps.
        if (WORKSPACE_PROTOCOL.test(String(range))) specifiers.add(specifier);
      }
    }
    for (const specifier of specifiers) {
      const direct = byName.get(specifier);
      if (direct) {
        if (direct.id !== node.id) node.dependsOn.push(direct.id);
        continue;
      }
      const overridden = overrides.get(specifier);
      const aliased = overridden
        ? [...byId.values()].find((candidate) => candidate.dir === overridden)
        : undefined;
      if (!aliased) {
        // Fail closed: an unresolvable workspace-protocol dependency means the
        // graph is wrong, and a wrong graph under-routes silently.
        throw new Error(`${node.dir} depends on unresolved workspace package ${specifier}`);
      }
      if (aliased.id !== node.id) node.dependsOn.push(aliased.id);
    }
    node.dependsOn = [...new Set(node.dependsOn)].sort();
  }

  const modules = [...byId.values()].sort((left, right) => left.id.localeCompare(right.id));
  for (const node of modules) {
    if (dependencyCycle(byId, node.id)) throw new Error(`workspace dependency cycle through ${node.id}`);
  }
  return { modules, byId, byName };
}

/** Transitive dependents (reverse edges), including the seeds themselves. */
export function dependentClosure(graph, seedIds) {
  const seeds = [...seedIds];
  for (const id of seeds) {
    if (!graph.byId.has(id)) throw new Error(`unknown module: ${id}`);
  }
  const closure = new Set(seeds);
  let changed = true;
  while (changed) {
    changed = false;
    for (const node of graph.modules) {
      if (closure.has(node.id)) continue;
      if (node.dependsOn.some((id) => closure.has(id))) {
        closure.add(node.id);
        changed = true;
      }
    }
  }
  return [...closure].sort();
}

/**
 * Which module owns a repository-relative path.
 *
 * A path owned by no module is REPO-WIDE, not "no impact": root tooling, CI
 * definitions, the lockfile, and shared scripts all change every module's build.
 */
export function classifyPath(graph, path) {
  const normalized = String(path).replace(/\\/g, "/").replace(/^\.\//, "");
  for (const node of graph.modules) {
    if (normalized === node.dir || normalized.startsWith(`${node.dir}/`)) return node.id;
  }
  return null;
}

export function routeChanges(graph, paths) {
  const seeds = new Set();
  const unowned = [];
  for (const path of paths) {
    if (String(path).trim() === "") continue;
    const owner = classifyPath(graph, path);
    if (owner === null) unowned.push(String(path).replace(/\\/g, "/"));
    else seeds.add(owner);
  }
  const repoWide = unowned.length > 0;
  const affected = repoWide ? graph.modules.map((node) => node.id) : [...seeds].sort();
  const closure = dependentClosure(graph, affected);
  return {
    repoWide,
    unowned: unowned.slice(0, 20).sort(),
    affected,
    closure,
    publish: closure.filter((id) => graph.byId.get(id).publishable)
  };
}

export function moduleMatrix(graph, ids) {
  return {
    include: ids.map((id) => {
      const node = graph.byId.get(id);
      return { id: node.id, name: node.name, dir: node.dir, version: node.version };
    })
  };
}

export function changedPathsFromGit(repoRoot, base, head) {
  const output = execFileSync(
    "git",
    ["diff", "--name-only", "--find-renames", "--no-renames", `${base}..${head}`],
    { cwd: repoRoot, encoding: "utf8" }
  );
  return output.split("\n").map((line) => line.trim()).filter((line) => line.length > 0);
}

export function parseArgs(argv) {
  const parsed = { base: "", head: "", paths: undefined, githubOutput: "", repoRoot: "" };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (value === undefined) throw new Error(`${flag} requires a value`);
    if (flag === "--base") parsed.base = value;
    else if (flag === "--head") parsed.head = value;
    else if (flag === "--paths") parsed.paths = value.split(/[\n,]/).map((entry) => entry.trim()).filter(Boolean);
    else if (flag === "--github-output") parsed.githubOutput = value;
    else if (flag === "--repo-root") parsed.repoRoot = value;
    else throw new Error(`unknown argument: ${flag}`);
    index += 1;
  }
  if (parsed.paths === undefined) {
    if (!SHA.test(parsed.base)) throw new Error(`--base must be a commit sha, got: ${parsed.base || "(missing)"}`);
    if (!SHA.test(parsed.head)) throw new Error(`--head must be a commit sha, got: ${parsed.head || "(missing)"}`);
  }
  return parsed;
}

/** Render the router verdict as `key=value` lines. Never partial: see the header. */
export function renderOutputs(result) {
  return [
    `routing_failed=${result.routingFailed ? "true" : "false"}`,
    `repo_wide=${result.repoWide ? "true" : "false"}`,
    `affected=${JSON.stringify(result.affected)}`,
    `closure=${JSON.stringify(result.closure)}`,
    `publish=${JSON.stringify(result.publish)}`,
    `checks_matrix=${JSON.stringify(result.checksMatrix)}`,
    `publish_matrix=${JSON.stringify(result.publishMatrix)}`,
    `has_publish=${result.publish.length > 0 ? "true" : "false"}`,
    `json=${JSON.stringify(result)}`,
    ""
  ].join("\n");
}

export function route({ repoRoot, base, head, paths }) {
  const graph = readModuleGraph(repoRoot);
  const changed = paths ?? changedPathsFromGit(repoRoot, base, head);
  const routed = routeChanges(graph, changed);
  return {
    routingFailed: false,
    repoWide: routed.repoWide,
    unowned: routed.unowned,
    affected: routed.affected,
    closure: routed.closure,
    publish: routed.publish,
    checksMatrix: moduleMatrix(graph, routed.closure),
    publishMatrix: moduleMatrix(graph, routed.publish),
    changedPaths: changed.length
  };
}

/**
 * The degraded verdict. Verification runs everything; publication is withheld,
 * because publishing an artifact nobody could route is the one irreversible act
 * in this pipeline.
 */
export function fallbackResult(repoRoot, reason) {
  let ids = [];
  let matrix = { include: [] };
  try {
    const graph = readModuleGraph(repoRoot);
    ids = graph.modules.map((node) => node.id);
    matrix = moduleMatrix(graph, ids);
  } catch {
    ids = [];
    matrix = { include: [] };
  }
  return {
    routingFailed: true,
    routingFailureReason: reason,
    repoWide: true,
    unowned: [],
    affected: ids,
    closure: ids,
    publish: [],
    checksMatrix: matrix,
    publishMatrix: { include: [] },
    changedPaths: 0
  };
}

export function main(argv = process.argv.slice(2)) {
  let githubOutput = "";
  let repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
  let result;
  try {
    const args = parseArgs(argv);
    githubOutput = args.githubOutput;
    if (args.repoRoot) repoRoot = resolve(args.repoRoot);
    result = route({ repoRoot, base: args.base, head: args.head, paths: args.paths });
  } catch (error) {
    const reason = error instanceof Error ? error.message : String(error);
    process.stderr.write(`::warning::public-module-graph routing failed: ${reason}\n`);
    result = fallbackResult(repoRoot, reason);
  }
  const rendered = renderOutputs(result);
  if (githubOutput) appendFileSync(githubOutput, rendered, "utf8");
  else process.stdout.write(rendered);
  return result;
}

function dependencyCycle(byId, startId) {
  const seen = new Set();
  const stack = [startId];
  while (stack.length > 0) {
    const current = stack.pop();
    for (const next of byId.get(current)?.dependsOn ?? []) {
      if (next === startId) return true;
      if (seen.has(next)) continue;
      seen.add(next);
      stack.push(next);
    }
  }
  return false;
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  main();
}
