// Finalize the CLI bundle:
//   1. esbuild bundles dist/cli.js (the tsc-built entrypoint) and its
//      workspace deps into a single ESM file.
//   2. Prepend the Bun shebang.
//   3. Write to dist/cli.mjs so the `bin` field works on Unix and the
//      ESM extension is unambiguous.
//   4. Compute a sha256 digest and write a sibling cli.mjs.sha256
//      used by release/package integrity checks.
//
// The published SDK copies ONLY dist/cli.mjs as the user-facing `aex`
// bin. Managed session internals use the separate runtime bridge artifact, so
// this host-install bundle stays independent of platform injection.
import { build } from "esbuild";
import { readFile, writeFile, chmod } from "node:fs/promises";
import { createHash } from "node:crypto";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const pkgRoot = resolve(here, "..");
const distDir = resolve(pkgRoot, "dist");
const inputPath = resolve(distDir, "cli.js");
const outputPath = resolve(distDir, "cli.mjs");
const digestPath = resolve(distDir, "cli.mjs.sha256");

const result = await build({
  entryPoints: [inputPath],
  bundle: true,
  // Bun runs the ESM output and built-ins, so esbuild's node platform remains
  // the most compatible resolver for package dependencies.
  platform: "node",
  format: "esm",
  target: "es2022",
  write: false,
  // Mark runtime built-ins external; we only bundle our own workspace deps.
  external: [
    "node:*",
    "fs",
    "fs/promises",
    "path",
    "url",
    "crypto",
    "buffer",
    "stream",
    "util"
  ],
  // The bundle is loaded as ESM at runtime; suppress sourcemaps so the
  // shipped artifact stays minimal and free of file-path leakage.
  sourcemap: false,
  legalComments: "none",
  minify: false
});

if (result.outputFiles.length !== 1) {
  throw new Error(`unexpected esbuild output count: ${result.outputFiles.length}`);
}
const bundled = result.outputFiles[0].text;

const shebang = "#!/usr/bin/env bun\n";
const final = shebang + bundled;
await writeFile(outputPath, final, "utf8");

try {
  // mode 0755; harmless on Windows.
  await chmod(outputPath, 0o755);
} catch {
  /* ignore */
}

const digest = createHash("sha256").update(final).digest("hex");
await writeFile(digestPath, `${digest}  cli.mjs\n`, "utf8");

console.log(`built ${outputPath} (${final.length} bytes)`);
console.log(`sha256 ${digest}`);
