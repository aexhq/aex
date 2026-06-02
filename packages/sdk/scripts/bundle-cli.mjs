// Copy the @antpath/cli ESM bundle (and its sha256 sidecar) into the
// SDK's dist directory so that the published `antpath` npm package's
// `bin: antpath` entry resolves to a real artifact at install time.
//
// The CLI is built from `packages/cli` (a workspace package) so
// the agent-first invariant — "one npm package, one CLI binary" — holds
// mechanically: subscribers run `npm i antpath` and immediately get
// both the SDK import and the `antpath` executable. The worker uses the
// same bundle in-container at /antpath/antpath.
//
// This script is invoked from the SDK's `build` script (which itself
// runs `@antpath/cli`'s build first), so by the time we run here, the
// source bundle is guaranteed to exist. If it does not we throw with a
// clear message so a misconfigured publish never silently ships an
// empty `bin`.

import { copyFile, mkdir, stat } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(here, "..");
const cliDistDir = resolve(sdkRoot, "..", "cli", "dist");
const sourceBundle = resolve(cliDistDir, "cli.mjs");
const sourceDigest = resolve(cliDistDir, "cli.mjs.sha256");
const destDir = resolve(sdkRoot, "dist");
const destBundle = resolve(destDir, "cli.mjs");
const destDigest = resolve(destDir, "cli.mjs.sha256");

async function fileExists(path) {
  try {
    const s = await stat(path);
    return s.isFile();
  } catch {
    return false;
  }
}

if (!(await fileExists(sourceBundle))) {
  throw new Error(
    `Missing CLI bundle at ${sourceBundle}. ` +
      `Build @antpath/cli before packing the SDK ` +
      `(e.g. \`pnpm --filter @antpath/cli run build\`).`
  );
}

await mkdir(destDir, { recursive: true });
await copyFile(sourceBundle, destBundle);
if (await fileExists(sourceDigest)) {
  await copyFile(sourceDigest, destDigest);
}

console.log(`bundled CLI: ${sourceBundle} -> ${destBundle}`);
