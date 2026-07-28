#!/usr/bin/env node

/**
 * Per-module canary identity.
 *
 * `canary-version.mjs` and `release-source.mjs` answer the same two questions for
 * the SDK alone. Once every public module publishes its own npm canary, those
 * answers become per-module, so this file generalises them WITHOUT forking the
 * implementation: the SDK path still runs `applySdkVersion` / `applyReleaseSource`
 * verbatim, because the SDK additionally carries a `SDK_VERSION` export that a
 * generic package.json writer would leave stale.
 *
 * Source-tag shape is a declared exception rather than a hidden one. The SDK keeps
 * the flat `canary/<version>` tag because the private release parses exactly that
 * shape (`platform/.github/workflows/deploy-dev.yml:83`, `deploy-prd.yml:99`
 * strip the `canary/` prefix to recover the SDK version). Every other module uses
 * `canary/<module>/<version>`, which cannot collide with it.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { buildCanaryVersion, applySdkVersion, isCanaryVersion } from "./canary-version.mjs";
import { applyReleaseSource } from "./release-source.mjs";
import { readModuleGraph } from "./public-module-graph.mjs";

const SOURCE_SHA = /^[0-9a-f]{40}$/;
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

/** Write the canary version and the exact source identity into a module manifest. */
export function applyModuleCanary(repoRoot, moduleId, { version, sha }) {
  if (!isCanaryVersion(version)) {
    throw new Error(`refusing to apply invalid canary version: ${version}`);
  }
  const normalizedSha = String(sha ?? "").toLowerCase();
  if (!SOURCE_SHA.test(normalizedSha)) throw new Error(`invalid release source SHA: ${sha ?? "(missing)"}`);

  const node = moduleNode(repoRoot, moduleId);
  if (moduleId === FLAT_SOURCE_TAG_MODULE) {
    applySdkVersion(repoRoot, version);
    applyReleaseSource(repoRoot, normalizedSha);
    return { module: moduleId, name: node.name, version, sourceSha: normalizedSha };
  }

  const manifestPath = resolve(repoRoot, node.dir, "package.json");
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  if (manifest.name !== node.name) {
    throw new Error(`unexpected package at ${manifestPath}: ${manifest.name ?? "(missing name)"}`);
  }
  manifest.version = version;
  manifest.aexRelease = { sourceSha: normalizedSha };
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  return { module: moduleId, name: node.name, version, sourceSha: normalizedSha };
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
  const upstream = {};
  for (const id of node.dependsOn) {
    const dependency = graph.byId.get(id);
    if (!dependency.publishable) continue;
    upstream[dependency.name] = dependency.version;
  }
  return upstream;
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
      sha: required(args, "sha")
    });
    process.stdout.write(`Applied ${applied.name}@${applied.version} from ${applied.sourceSha}.\n`);
    return applied;
  }
  if (args.command === "upstream") {
    const upstream = moduleUpstreamVersions(repoRoot, required(args, "module"));
    process.stdout.write(`${JSON.stringify(upstream)}\n`);
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
