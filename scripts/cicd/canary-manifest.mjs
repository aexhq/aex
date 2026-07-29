#!/usr/bin/env node

/**
 * The public canary manifest — the release identity of one main push.
 *
 * Each affected public module publishes its own npm canary independently, so
 * "what did we test" is no longer one commit and no longer one package version.
 * It is the SET of package versions, each with the integrity value the registry
 * actually served and the source commit it was built from. That set is this file's
 * output, and it is the only thing the private release is allowed to consume:
 * following the public repository's current `main` would pin a tree nobody tested
 * together.
 *
 * Every field here is release identity, not incidental lockfile data:
 *
 *   version    exact, immutable, never a range and never a dist-tag.
 *   integrity  the registry's own sha512 for the published tarball.
 *   sourceSha  the exact commit the module was built and tested at.
 *   sourceTag  the immutable git tag that pins that commit.
 *   upstream   the exact public package versions this module was built against —
 *              a canary built against different contracts is a different artifact
 *              at the same commit, so the consumer can verify the pair.
 *
 * The private repository's SELECTED manifest is a different schema
 * (`aex.selected-canary-manifest.v1`) because it adds the consumer closure, which
 * public CI cannot know. The two are deliberately not interchangeable.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { readModuleGraph } from "./public-module-graph.mjs";

export const CANARY_MANIFEST_SCHEMA = "aex.public-canary-manifest.v1";

const SOURCE_SHA = /^[0-9a-f]{40}$/;
const INTEGRITY = /^sha512-[A-Za-z0-9+/]+={0,2}$/;
const EXACT_VERSION = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
const PACKAGE_NAME = /^(?:@[a-z0-9-][a-z0-9._-]*\/)?[a-z0-9-][a-z0-9._-]*$/;
const REPOSITORY = /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/;

export function parseCanaryManifest(value) {
  const manifest = object(value, "manifest");
  exact(manifest.schema, CANARY_MANIFEST_SCHEMA, "manifest.schema");
  regex(manifest.repository, REPOSITORY, "manifest.repository");
  regex(manifest.sourceSha, SOURCE_SHA, "manifest.sourceSha");
  timestamp(manifest.generatedAt, "manifest.generatedAt");
  nonEmptyString(manifest.workflowRunId, "manifest.workflowRunId");

  const packages = array(manifest.packages, "manifest.packages");
  if (packages.length === 0) fail("manifest.packages must not be empty");

  const byName = new Map();
  for (let index = 0; index < packages.length; index += 1) {
    const entry = object(packages[index], `manifest.packages[${index}]`);
    const label = `manifest.packages[${String(entry.name ?? index)}]`;
    nonEmptyString(entry.module, `${label}.module`);
    regex(entry.name, PACKAGE_NAME, `${label}.name`);
    regex(entry.version, EXACT_VERSION, `${label}.version`);
    exact(entry.npmDistTag, "canary", `${label}.npmDistTag`);
    regex(entry.integrity, INTEGRITY, `${label}.integrity`);
    regex(entry.sourceRepository, REPOSITORY, `${label}.sourceRepository`);
    regex(entry.sourceSha, SOURCE_SHA, `${label}.sourceSha`);
    nonEmptyString(entry.sourceTag, `${label}.sourceTag`);
    if (entry.sourceSha !== manifest.sourceSha) {
      // One manifest is one tested tree. A package built from a different commit
      // belongs to a different release, not to this one.
      fail(`${label}.sourceSha ${entry.sourceSha} differs from manifest.sourceSha ${manifest.sourceSha}`);
    }
    object(entry.upstream, `${label}.upstream`);
    for (const [name, version] of Object.entries(entry.upstream)) {
      regex(name, PACKAGE_NAME, `${label}.upstream key`);
      regex(version, EXACT_VERSION, `${label}.upstream.${name}`);
    }
    if (byName.has(entry.name)) fail(`duplicate package in manifest: ${entry.name}`);
    byName.set(entry.name, entry);
  }

  // Every recorded upstream must be an immutable package from this same run.
  for (const entry of byName.values()) {
    for (const [name, version] of Object.entries(entry.upstream)) {
      const published = byName.get(name);
      if (!published) {
        fail(`${entry.name} records upstream ${name}@${version}, but this release did not publish ${name}`);
      }
      if (published.version !== version) {
        fail(
          `${entry.name} records upstream ${name}@${version}, but this release published ${name}@${published.version}`
        );
      }
    }
  }

  return manifest;
}

/**
 * @param {{
 *   repoRoot: string,
 *   repository: string,
 *   sourceSha: string,
 *   workflowRunId: string,
 *   generatedAt: string,
 *   entries: readonly { module: string, name: string, version: string, integrity: string, sourceTag: string }[]
 * }} options
 */
export function buildCanaryManifest(options) {
  const graph = readModuleGraph(options.repoRoot);

  const packages = [...options.entries]
    .sort((left, right) => left.name.localeCompare(right.name))
    .map((entry) => {
      const node = graph.byId.get(entry.module);
      if (!node) throw new Error(`unknown public module in release entries: ${entry.module}`);
      if (node.name !== entry.name) {
        throw new Error(`release entry ${entry.module} claims package ${entry.name}, workspace says ${node.name}`);
      }
      const expectedNames = node.dependsOn
        .map((id) => graph.byId.get(id))
        .filter((dependency) => dependency.publishable)
        .map((dependency) => dependency.name)
        .sort();
      const actualNames = Object.keys(entry.upstream ?? {}).sort();
      if (JSON.stringify(actualNames) !== JSON.stringify(expectedNames)) {
        throw new Error(
          `release entry ${entry.module} upstream packages ${JSON.stringify(actualNames)} ` +
            `do not match workspace dependencies ${JSON.stringify(expectedNames)}`
        );
      }
      return {
        module: entry.module,
        name: entry.name,
        version: entry.version,
        npmDistTag: "canary",
        integrity: entry.integrity,
        sourceRepository: options.repository,
        sourceSha: options.sourceSha,
        sourceTag: entry.sourceTag,
        upstream: entry.upstream
      };
    });

  return parseCanaryManifest({
    schema: CANARY_MANIFEST_SCHEMA,
    repository: options.repository,
    sourceSha: options.sourceSha,
    generatedAt: options.generatedAt,
    workflowRunId: options.workflowRunId,
    packages
  });
}

/** Merge the per-module release records each publish job uploaded. */
export function readReleaseEntries(paths) {
  const entries = [];
  for (const path of paths) {
    const record = JSON.parse(readFileSync(path, "utf8"));
    entries.push({
      module: record.module,
      name: record.name,
      version: record.version,
      integrity: record.npmIntegrity ?? record.integrity,
      sourceTag: record.sourceTag,
      upstream: record.upstream
    });
  }
  return entries;
}

function parseArgs(argv) {
  const [command, ...rest] = argv;
  const args = { command, records: [] };
  for (let index = 0; index < rest.length; index += 1) {
    const key = rest[index];
    if (!key?.startsWith("--")) throw new Error(`unexpected argument: ${key ?? "(missing)"}`);
    const value = rest[++index];
    if (value === undefined) throw new Error(`${key} requires a value`);
    const field = key.slice(2).replace(/-([a-z])/g, (_match, char) => char.toUpperCase());
    if (field === "record") args.records.push(value);
    else args[field] = value;
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
  const repoRoot = resolve(args.repoRoot || resolve(dirname(fileURLToPath(import.meta.url)), "..", ".."));

  if (args.command === "build") {
    if (args.records.length === 0) throw new Error("at least one --record is required");
    const manifest = buildCanaryManifest({
      repoRoot,
      repository: required(args, "repository"),
      sourceSha: required(args, "sourceSha"),
      workflowRunId: required(args, "workflowRunId"),
      generatedAt: args.generatedAt || new Date().toISOString(),
      entries: readReleaseEntries(args.records)
    });
    const rendered = `${JSON.stringify(manifest, null, 2)}\n`;
    if (args.out) writeFileSync(resolve(args.out), rendered, "utf8");
    else process.stdout.write(rendered);
    return manifest;
  }
  if (args.command === "check") {
    const manifest = parseCanaryManifest(JSON.parse(readFileSync(resolve(required(args, "manifest")), "utf8")));
    process.stdout.write(
      `canary manifest ok: ${manifest.packages.length} package(s) at ${manifest.sourceSha}\n`
    );
    return manifest;
  }
  throw new Error(`unknown command: ${args.command ?? "(missing)"}`);
}

function object(value, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) fail(`${label} must be an object`);
  return value;
}

function array(value, label) {
  if (!Array.isArray(value)) fail(`${label} must be an array`);
  return value;
}

function nonEmptyString(value, label) {
  if (typeof value !== "string" || value.trim().length === 0) fail(`${label} must be a non-empty string`);
  return value;
}

function regex(value, pattern, label) {
  nonEmptyString(value, label);
  if (!pattern.test(value)) fail(`${label} has an invalid format: ${value}`);
  return value;
}

function exact(value, expected, label) {
  if (value !== expected) fail(`${label} must equal ${JSON.stringify(expected)}`);
  return value;
}

function timestamp(value, label) {
  nonEmptyString(value, label);
  const milliseconds = Date.parse(value);
  if (!Number.isFinite(milliseconds) || new Date(milliseconds).toISOString() !== value) {
    fail(`${label} must be a canonical ISO-8601 timestamp`);
  }
  return value;
}

function fail(message) {
  throw new Error(`canary manifest: ${message}`);
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`::error::${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  }
}
