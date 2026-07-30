#!/usr/bin/env node

/**
 * Per-module canary identity.
 *
 * `canary-version.mjs` and `release-source.mjs` answer the same two questions for
 * the SDK alone. Once every public module publishes its own npm canary, those
 * answers become per-module, so this file generalises them WITHOUT forking the
 * implementation: the SDK path still runs `applySdkVersion` /
 * `applyReleaseSource` verbatim because its source tag and registry metadata
 * remain the release selector used by the hosted deployment.
 *
 * Source-tag shape is a declared exception rather than a hidden one. The SDK keeps
 * the flat `canary/<version>` tag because the private release parses exactly that
 * shape (`platform/.github/workflows/deploy-dev.yml:83`, `deploy-prd.yml:99`
 * strip the `canary/` prefix to recover the SDK version). Every other module uses
 * `canary/<module>/<version>`, which cannot collide with it.
 */

import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { buildCanaryVersion, applySdkVersion, isCanaryVersion } from "./canary-version.mjs";
import { applyReleaseSource } from "./release-source.mjs";
import { readModuleGraph } from "./public-module-graph.mjs";

const SOURCE_SHA = /^[0-9a-f]{40}$/;
const DEPENDENCY_FIELDS = ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"];
/** The one module whose source tag is flat, and the reason it is. See the header. */
const FLAT_SOURCE_TAG_MODULE = "sdk";

export function moduleNode(repoRoot, moduleId) {
  const graph = readModuleGraph(repoRoot);
  const node = graph.byId.get(moduleId);
  if (!node) throw new Error(`unknown public module: ${moduleId}`);
  if (!node.publishable) throw new Error(`module ${moduleId} is private and must not be published`);
  return node;
}

export function resolveModuleCanaryVersion(repoRoot, moduleId, sha, run) {
  const node = moduleNode(repoRoot, moduleId);
  return buildCanaryVersion({ baseVersion: node.version, sha, run });
}

export function moduleSourceTag(moduleId, version) {
  if (!isCanaryVersion(version)) throw new Error(`invalid canary version: ${version}`);
  return moduleId === FLAT_SOURCE_TAG_MODULE ? `canary/${version}` : `canary/${moduleId}/${version}`;
}

/** Write the canary version and exact source/upstream identity into a module manifest. */
export function applyModuleCanary(repoRoot, moduleId, { version, sha, run }) {
  if (!isCanaryVersion(version)) {
    throw new Error(`refusing to apply invalid canary version: ${version}`);
  }
  const normalizedSha = String(sha ?? "").toLowerCase();
  if (!SOURCE_SHA.test(normalizedSha)) throw new Error(`invalid release source SHA: ${sha ?? "(missing)"}`);

  const node = moduleNode(repoRoot, moduleId);
  const upstream = moduleUpstreamCanaryVersions(repoRoot, moduleId, normalizedSha, run);
  if (moduleId === FLAT_SOURCE_TAG_MODULE) {
    applySdkVersion(repoRoot, version);
    applyReleaseSource(repoRoot, normalizedSha);
  }

  const manifestPath = resolve(repoRoot, node.dir, "package.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  if (manifest.name !== node.name) {
    throw new Error(`unexpected package at ${manifestPath}: ${manifest.name ?? "(missing name)"}`);
  }
  manifest.version = version;
  manifest.aexRelease = { sourceSha: normalizedSha, upstream };
  for (const field of DEPENDENCY_FIELDS) {
    for (const name of Object.keys(manifest[field] ?? {})) {
      if (upstream[name]) manifest[field][name] = upstream[name];
    }
  }
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  return { module: moduleId, name: node.name, version, sourceSha: normalizedSha, upstream };
}

/**
 * The exact upstream public package versions this module was built against.
 *
 * Release identity, not incidental lockfile data: a canary built against a
 * different `@aexhq/contracts` is a different artifact even at the same source
 * commit, so the manifest records it and the private consumer can verify it.
 */
export function moduleUpstreamVersions(repoRoot, moduleId) {
  const graph = readModuleGraph(repoRoot);
  const node = graph.byId.get(moduleId);
  if (!node) throw new Error(`unknown public module: ${moduleId}`);
  const manifest = JSON.parse(readFileSync(resolve(repoRoot, node.dir, "package.json"), "utf8"));
  const upstream = {};
  for (const field of DEPENDENCY_FIELDS) {
    for (const [name, declaredVersion] of Object.entries(manifest[field] ?? {})) {
      const dependency = graph.byName.get(name);
      if (dependency?.publishable) {
        upstream[name] = String(declaredVersion).startsWith("workspace:")
          ? dependency.version
          : String(declaredVersion);
      }
    }
  }
  return Object.fromEntries(Object.entries(upstream).sort(([left], [right]) => left.localeCompare(right)));
}

export function moduleUpstreamCanaryVersions(repoRoot, moduleId, sha, run) {
  const graph = readModuleGraph(repoRoot);
  const stable = moduleUpstreamVersions(repoRoot, moduleId);
  return Object.fromEntries(
    Object.keys(stable).map((name) => {
      const dependency = graph.byName.get(name);
      return [name, buildCanaryVersion({ baseVersion: dependency.version, sha, run })];
    })
  );
}

/** Verify the metadata that was actually written into a packed tarball. */
export function verifyPackedModuleManifest(repoRoot, moduleId, manifest, { version, sha, run }) {
  const node = moduleNode(repoRoot, moduleId);
  if (manifest?.name !== node.name) throw new Error(`packed package name is ${manifest?.name}, expected ${node.name}`);
  if (manifest.version !== version) {
    throw new Error(`packed ${node.name} version is ${manifest.version}, expected ${version}`);
  }
  const expectedUpstream = moduleUpstreamCanaryVersions(repoRoot, moduleId, sha, run);
  if (manifest.aexRelease?.sourceSha !== sha) {
    throw new Error(`packed ${node.name} source SHA does not match ${sha}`);
  }
  if (JSON.stringify(manifest.aexRelease?.upstream) !== JSON.stringify(expectedUpstream)) {
    throw new Error(`packed ${node.name} upstream identity does not match this run`);
  }
  for (const field of DEPENDENCY_FIELDS) {
    for (const [name, expected] of Object.entries(expectedUpstream)) {
      if (manifest[field]?.[name] !== undefined && manifest[field][name] !== expected) {
        throw new Error(`packed ${node.name} ${field}.${name} is ${manifest[field][name]}, expected ${expected}`);
      }
    }
  }
  return expectedUpstream;
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  const args = { command };
  for (let index = 0; index < rest.length; index += 1) {
    const key = rest[index];
    if (!key?.startsWith("--")) throw new Error(`unexpected argument: ${key ?? "(missing)"}`);
    const value = rest[++index];
    if (value === undefined) throw new Error(`${key} requires a value`);
    args[key.slice(2).replace(/-([a-z])/g, (_match, char) => char.toUpperCase())] = value;
  }
  return args;
}

function required(args, key) {
  const value = args[key];
  if (!value) throw new Error(`--${key.replace(/[A-Z]/g, (char) => `-${char.toLowerCase()}`)} is required`);
  return value;
}

export function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  const repoRoot = resolve(args.repoRoot || defaultRepoRoot());

  if (args.command === "resolve") {
    const version = resolveModuleCanaryVersion(
      repoRoot,
      required(args, "module"),
      required(args, "sha"),
      required(args, "run")
    );
    process.stdout.write(`${version}\n`);
    return version;
  }
  if (args.command === "source-tag") {
    const tag = moduleSourceTag(required(args, "module"), required(args, "version"));
    process.stdout.write(`${tag}\n`);
    return tag;
  }
  if (args.command === "apply") {
    const applied = applyModuleCanary(repoRoot, required(args, "module"), {
      version: required(args, "version"),
      sha: required(args, "sha"),
      run: required(args, "run")
    });
    process.stdout.write(`Applied ${applied.name}@${applied.version} from ${applied.sourceSha}.\n`);
    return applied;
  }
  if (args.command === "upstream") {
    const upstream = moduleUpstreamVersions(repoRoot, required(args, "module"));
    process.stdout.write(`${JSON.stringify(upstream)}\n`);
    return upstream;
  }
  if (args.command === "upstream-lines") {
    const upstream = moduleUpstreamVersions(repoRoot, required(args, "module"));
    for (const [name, version] of Object.entries(upstream)) process.stdout.write(`${name}\t${version}\n`);
    return upstream;
  }
  if (args.command === "verify-packed") {
    const upstream = verifyPackedModuleManifest(
      repoRoot,
      required(args, "module"),
      JSON.parse(readFileSync(resolve(required(args, "manifest")), "utf8")),
      {
        version: required(args, "version"),
        sha: required(args, "sha"),
        run: required(args, "run")
      }
    );
    if (args.githubOutput) appendFileSync(args.githubOutput, `upstream=${JSON.stringify(upstream)}\n`, "utf8");
    process.stdout.write(`Verified packed ${required(args, "module")} metadata.\n`);
    return upstream;
  }
  throw new Error(`unknown command: ${args.command ?? "(missing)"}`);
}

function defaultRepoRoot() {
  return resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`::error::${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  }
}
