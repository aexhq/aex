import { readdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Asserts the shared sidebar support footer is present on every exported page.
// Runs as a postbuild step so a regression in the shared layout fails the docs build.
const scriptDir = dirname(fileURLToPath(import.meta.url));
const outRoot = resolve(scriptDir, "..", "out");
const SUPPORT_MAILTO = "mailto:support@aex.dev";

if (!existsSync(outRoot)) {
  console.error(`[support-footer] missing export directory: ${outRoot}. Run \`next build\` first.`);
  process.exit(1);
}

async function collectHtml(dir) {
  const files = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) files.push(...(await collectHtml(full)));
    else if (entry.name.endsWith(".html")) files.push(full);
  }
  return files;
}

const htmlFiles = await collectHtml(outRoot);
if (htmlFiles.length === 0) {
  console.error(`[support-footer] no HTML pages found under ${outRoot}.`);
  process.exit(1);
}

const missing = [];
for (const file of htmlFiles) {
  const html = await readFile(file, "utf8");
  if (!html.includes(SUPPORT_MAILTO)) missing.push(file);
}

if (missing.length > 0) {
  console.error(`[support-footer] ${SUPPORT_MAILTO} missing on ${missing.length} page(s):`);
  for (const file of missing) console.error(`  - ${file}`);
  process.exit(1);
}

console.log(`[support-footer] ${SUPPORT_MAILTO} present on all ${htmlFiles.length} exported page(s).`);
