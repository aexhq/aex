#!/usr/bin/env node

/**
 * Verify one selected canary against the registry before anything promotes it.
 *
 * Promotion is the point at which a canary stops being an artifact and starts
 * being what `npm install` hands a customer, so it must not trust the manifest
 * alone. The manifest says what CI believed it published; the registry says what
 * is actually installable now. This compares the two and refuses on any drift:
 *
 *  - the version must exist at the exact string in the manifest;
 *  - the registry's `dist.integrity` must equal the manifest's — a republished or
 *    tampered tarball changes it;
 *  - the version must still be the one carrying the `canary` dist-tag lineage,
 *    never something already promoted or already deprecated.
 *
 * Read-only. It fetches registry metadata and exits; it never mutates a tag.
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { parseCanaryManifest } from "./canary-manifest.mjs";

const DEFAULT_REGISTRY = "https://registry.npmjs.org";

export function selectManifestEntry(manifest, moduleId, version) {
  const entry = manifest.packages.find((candidate) => candidate.module === moduleId);
  if (!entry) {
    throw new Error(
      `manifest has no module ${moduleId}; it published: ${manifest.packages.map((item) => item.module).join(", ")}`
    );
  }
  if (entry.version !== version) {
    throw new Error(`manifest records ${entry.name}@${entry.version}, not ${version}`);
  }
  return entry;
}

export async function verifyCanarySelection(options, dependencies = {}) {
  const fetchImpl = dependencies.fetch ?? fetch;
  const registry = (options.registry ?? process.env.NPM_REGISTRY_URL ?? DEFAULT_REGISTRY).replace(/\/$/, "");
  const manifest = parseCanaryManifest(options.manifest);
  const entry = selectManifestEntry(manifest, options.module, options.version);

  const response = await fetchImpl(`${registry}/${encodeURIComponent(entry.name)}`, {
    headers: { accept: "application/json", "cache-control": "no-cache" }
  });
  if (!response.ok) throw new Error(`registry metadata for ${entry.name} returned HTTP ${response.status}`);
  const packument = await response.json();

  const published = packument.versions?.[entry.version];
  if (!published) throw new Error(`${entry.name}@${entry.version} is not published`);
  const integrity = published.dist?.integrity;
  if (integrity !== entry.integrity) {
    throw new Error(
      `${entry.name}@${entry.version} integrity drift: registry ${integrity ?? "(missing)"}, manifest ${entry.integrity}`
    );
  }

  const distTags = packument["dist-tags"] ?? {};
  if (distTags[options.distTag] === entry.version) {
    throw new Error(`${entry.name}@${entry.version} already carries dist-tag ${options.distTag}`);
  }
  if (distTags.canary !== entry.version) {
    // Not fatal to correctness, but it means the selection is not the canary the
    // release train most recently produced — say so rather than promote quietly.
    throw new Error(
      `${entry.name} canary dist-tag is ${distTags.canary ?? "(unset)"}, not the selected ${entry.version}`
    );
  }

  return {
    name: entry.name,
    module: entry.module,
    version: entry.version,
    integrity: entry.integrity,
    sourceSha: entry.sourceSha,
    sourceTag: entry.sourceTag,
    upstream: entry.upstream,
    currentDistTags: distTags
  };
}

function parseArgs(argv) {
  const args = { distTag: "latest" };
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index];
    if (!key?.startsWith("--")) throw new Error(`unexpected argument: ${key ?? "(missing)"}`);
    const value = argv[++index];
    if (value === undefined) throw new Error(`${key} requires a value`);
    args[key.slice(2).replace(/-([a-z])/g, (_match, char) => char.toUpperCase())] = value;
  }
  for (const key of ["manifest", "module", "version"]) {
    if (!args[key]) throw new Error(`--${key} is required`);
  }
  return args;
}

export async function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  const verified = await verifyCanarySelection({
    manifest: JSON.parse(readFileSync(resolve(args.manifest), "utf8")),
    module: args.module,
    version: args.version,
    distTag: args.distTag,
    registry: args.registry
  });
  process.stdout.write(`${JSON.stringify(verified, null, 2)}\n`);
  return verified;
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  main().catch((error) => {
    process.stderr.write(`::error::${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  });
}
