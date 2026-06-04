// Inline `@aexhq/contracts` into the SDK's `dist/` so the published
// `aex` npm tarball is fully self-contained.
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
//      dist/*.js and dist/*.d.ts to `from "./_contracts/index.js"`.
//      Files inside _contracts/ already use relative imports between
//      siblings so no further rewriting is needed there.
//   3. Sanity-check: no `@aexhq/contracts` string survives in the SDK's
//      dist (excluding the _contracts/ directory itself, where the
//      substring is allowed to appear in comments). Throw loud if any
//      leaked — that means we'd ship another broken tarball.
//
// Pair this with keeping `@aexhq/contracts` in devDependencies in
// packages/sdk/package.json — npm strips devDependencies from the
// published tarball, so the rewritten dist has zero external workspace
// references at install time.
//
// Idempotency: safe to run multiple times. The _contracts/ directory is
// removed before each copy, and the rewrite is a no-op on already
// rewritten files (the source string is gone after the first pass).

import { cp, mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(here, "..");
const contractsDistDir = resolve(sdkRoot, "..", "contracts", "dist");
const contractsIndex = resolve(contractsDistDir, "index.js");
const sdkDistDir = resolve(sdkRoot, "dist");
const inlinedDir = resolve(sdkDistDir, "_contracts");

const CONTRACTS_IMPORT_SPECIFIER = "@aexhq/contracts";
const REWRITE_TARGET = "./_contracts/index.js";

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
      `(e.g. \`pnpm --filter @aexhq/contracts run build\`).`
  );
}

// 1. Copy contracts dist into the SDK's _contracts/, skipping sourcemap
//    files. `cp` with a filter walks the tree exactly once.
await rm(inlinedDir, { recursive: true, force: true });
await mkdir(inlinedDir, { recursive: true });
await cp(contractsDistDir, inlinedDir, {
  recursive: true,
  filter: (src) => {
    const rel = relative(contractsDistDir, src);
    if (rel === "") return true;
    if (src.endsWith(".js.map") || src.endsWith(".d.ts.map") || src.endsWith(".tsbuildinfo")) {
      return false;
    }
    return true;
  },
});

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

const FROM_CONTRACTS = new RegExp(`from\\s+["']${CONTRACTS_IMPORT_SPECIFIER}["']`, "g");
const IMPORT_BARE_CONTRACTS = new RegExp(`import\\s+["']${CONTRACTS_IMPORT_SPECIFIER}["']`, "g");

const rewritten = [];
for (const file of await listSdkDistRoots()) {
  const before = await readFile(file, "utf8");
  if (!before.includes(CONTRACTS_IMPORT_SPECIFIER)) continue;
  const after = before
    .replace(FROM_CONTRACTS, `from "${REWRITE_TARGET}"`)
    .replace(IMPORT_BARE_CONTRACTS, `import "${REWRITE_TARGET}"`);
  if (after === before) continue;
  await writeFile(file, after, "utf8");
  rewritten.push(relative(sdkDistDir, file));
}

// 3. Sanity-check: no bare @aexhq/contracts specifier survived in the
//    SDK dist root (sourcemaps may still reference it as a string but
//    we removed those above).
async function scanForLeaks() {
  const leaked = [];
  for (const file of await listSdkDistRoots()) {
    const contents = await readFile(file, "utf8");
    if (contents.includes(`"${CONTRACTS_IMPORT_SPECIFIER}"`)) {
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
    `(rewrote ${rewritten.length} file${rewritten.length === 1 ? "" : "s"}: ${rewritten.join(", ")})`
);
