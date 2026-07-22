// Inline `@aexhq/contracts` into the SDK's `dist/` so the published
// SDK tarball is fully self-contained.
//
// Why this script exists
// ----------------------
// The SDK source imports from `@aexhq/contracts`, which publishes in
// lockstep with `aex`. The SDK still inlines its dist output so a
// local packed SDK tarball can be clean-installed without staging a
// companion contracts tarball first.
//
// The fix (after tsc emit):
//   1. Copy packages/contracts/dist/** -> packages/sdk/dist/_contracts/**,
//      skipping sourcemap / tsbuildinfo files (point at source paths
//      that are not in the tarball anyway).
//   2. Rewrite every `from "@aexhq/contracts"` in the SDK's emitted
//      dist/*.js and dist/*.d.ts to `from "./_contracts/index.js"`,
//      and every supported contracts subpath to its inlined sibling.
//      Files inside _contracts/ already use relative imports between
//      siblings so no further rewriting is needed there.
//   3. Sanity-check: no `@aexhq/contracts` string survives in the SDK's
//      dist (excluding the _contracts/ directory itself, where the
//      substring is allowed to appear in comments). Throw loud if any
//      leaked — that means we'd ship another broken tarball.
//
// Pair this with keeping `@aexhq/contracts` in devDependencies in
// packages/sdk/package.json. Consumer installs do not install devDependencies,
// so the rewritten dist has zero external workspace references at install time.
//
// Idempotency: safe to run multiple times. The _contracts/ directory is
// removed before each copy, and the rewrite is a no-op on already
// rewritten files (the source string is gone after the first pass).

import { cp, mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { dirname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(here, "..");
const contractsDistDir = resolve(sdkRoot, "..", "contracts", "dist");
const contractsIndex = resolve(contractsDistDir, "index.js");
const sdkDistDir = resolve(sdkRoot, "dist");
const inlinedDir = resolve(sdkDistDir, "_contracts");

const CONTRACTS_IMPORT_SPECIFIER = "@aexhq/contracts";
const CONTRACTS_IMPORT_PREFIX = `${CONTRACTS_IMPORT_SPECIFIER}/`;
const RUNNER_ONLY_CONTRACT_MODULES = new Set([
  "subagent-input.d.ts",
  "subagent-input.js",
  "subagent-runtime.d.ts",
  "subagent-runtime.js"
]);

async function fileExists(path) {
  try {
    const s = await stat(path);
    return s.isFile();
  } catch {
    return false;
  }
}

if (!(await fileExists(contractsIndex))) {
  throw new Error(
    `Missing @aexhq/contracts dist at ${contractsIndex}. ` +
      `Build the contracts package before packing the SDK ` +
      `(e.g. \`bun run --cwd ../.. --filter @aexhq/contracts build\`).`
  );
}

// 1. Copy contracts dist into the SDK's _contracts/, skipping sourcemap
//    files. `cp` with a filter walks the tree exactly once.
await rm(inlinedDir, { recursive: true, force: true });
await mkdir(inlinedDir, { recursive: true });
await cp(contractsDistDir, inlinedDir, {
  recursive: true,
  filter: (src) => {
    const rel = relative(contractsDistDir, src).replaceAll("\\", "/");
    if (rel === "") return true;
    if (RUNNER_ONLY_CONTRACT_MODULES.has(rel)) return false;
    if (src.endsWith(".js.map") || src.endsWith(".d.ts.map") || src.endsWith(".tsbuildinfo")) {
      return false;
    }
    return true;
  },
});

async function listFilesRecursive(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...await listFilesRecursive(full));
    } else if (entry.isFile()) {
      files.push(full);
    }
  }
  return files;
}

const strippedSourceMaps = [];
for (const file of await listFilesRecursive(inlinedDir)) {
  if (!file.endsWith(".js")) continue;
  const before = await readFile(file, "utf8");
  const after = before.replace(/(?:\r?\n)?\/\/# sourceMappingURL=.*(?:\r?\n)?$/u, "\n");
  if (after === before) continue;
  await writeFile(file, after, "utf8");
  strippedSourceMaps.push(relative(inlinedDir, file));
}

// 2. Rewrite SDK dist files that import from @aexhq/contracts. We only
//    look at the dist root (one level), not _contracts/ — those files
//    came from packages/contracts/dist and don't reference the bare
//    specifier.
async function listSdkDistRoots() {
  const entries = await readdir(sdkDistDir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (!entry.isFile()) continue;
    if (entry.name === "_contracts") continue;
    if (entry.name.endsWith(".js") || entry.name.endsWith(".d.ts")) {
      files.push(resolve(sdkDistDir, entry.name));
    }
  }
  return files;
}

async function rewriteTargetForSpecifier(specifier) {
  if (specifier === CONTRACTS_IMPORT_SPECIFIER) return "./_contracts/index.js";
  if (!specifier.startsWith(CONTRACTS_IMPORT_PREFIX)) return null;
  const subpath = specifier.slice(CONTRACTS_IMPORT_PREFIX.length);
  if (!/^[A-Za-z0-9._/-]+$/.test(subpath) || subpath.includes("..")) {
    throw new Error(`inline-contracts: refusing unsupported contracts subpath ${specifier}`);
  }
  const target = resolve(inlinedDir, `${subpath}.js`);
  const rel = relative(inlinedDir, target);
  if (rel.startsWith("..") || rel.includes(`..${sep}`) || rel === "") {
    throw new Error(`inline-contracts: contracts subpath escapes inlined dir: ${specifier}`);
  }
  if (!(await fileExists(target))) {
    throw new Error(`inline-contracts: ${specifier} maps to missing inlined module ${relative(sdkDistDir, target)}`);
  }
  return `./_contracts/${subpath}.js`;
}

async function rewriteContractsSpecifier(match, quote, specifier) {
  const target = await rewriteTargetForSpecifier(specifier);
  if (target === null) return match;
  return match.replace(`${quote}${specifier}${quote}`, `${quote}${target}${quote}`);
}

async function rewriteContractsSpecifiers(text) {
  let out = "";
  let cursor = 0;
  const pattern = /\b(?:from|import)\s+(["'])(@aexhq\/contracts(?:\/[A-Za-z0-9._/-]+)?)\1/g;
  for (const match of text.matchAll(pattern)) {
    out += text.slice(cursor, match.index);
    out += await rewriteContractsSpecifier(match[0], match[1], match[2]);
    cursor = match.index + match[0].length;
  }
  return out + text.slice(cursor);
}

const rewritten = [];
for (const file of await listSdkDistRoots()) {
  const before = await readFile(file, "utf8");
  if (!before.includes(CONTRACTS_IMPORT_SPECIFIER)) continue;
  const after = await rewriteContractsSpecifiers(before);
  if (after === before) continue;
  await writeFile(file, after, "utf8");
  rewritten.push(relative(sdkDistDir, file));
}

// 3. Sanity-check: no @aexhq/contracts specifier survived in the
//    SDK dist root (sourcemaps may still reference it as a string but
//    we removed those above).
async function scanForLeaks() {
  const leaked = [];
  for (const file of await listSdkDistRoots()) {
    const contents = await readFile(file, "utf8");
    if (contents.match(/["']@aexhq\/contracts(?:\/[^"']*)?["']/)) {
      leaked.push(relative(sdkDistDir, file));
    }
  }
  return leaked;
}

const leaks = await scanForLeaks();
if (leaks.length > 0) {
  throw new Error(
    `inline-contracts: ${CONTRACTS_IMPORT_SPECIFIER} still referenced after rewrite in:\n` +
      leaks.map((f) => `  - ${f}`).join("\n") +
      `\nThe published tarball would 404 at install time. Investigate before publishing.`
  );
}

console.log(
  `inlined @aexhq/contracts into ${relative(sdkRoot, inlinedDir)} ` +
    `(rewrote ${rewritten.length} file${rewritten.length === 1 ? "" : "s"}: ${rewritten.join(", ")}; ` +
    `stripped sourcemap comments from ${strippedSourceMaps.length} file${strippedSourceMaps.length === 1 ? "" : "s"})`
);
