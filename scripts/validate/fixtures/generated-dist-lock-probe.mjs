import { access, mkdir, readdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

const root = process.argv[2];
const holdMs = Number(process.argv[3] ?? 100);
const releasePath = process.argv[4];
const markerDir = join(root, "active");
await mkdir(markerDir, { recursive: true });
const marker = join(markerDir, String(process.pid));
await writeFile(marker, "active", "utf8");
try {
  await recordOverlap();
  if (releasePath) await waitForRelease(releasePath);
  else await new Promise((resolve) => setTimeout(resolve, holdMs));
  await recordOverlap();
} finally {
  await rm(marker, { force: true });
}

async function recordOverlap() {
  if ((await readdir(markerDir)).length > 1) {
    await writeFile(join(root, `overlap-${process.pid}`), "overlap", "utf8");
  }
}

async function waitForRelease(path) {
  const deadline = Date.now() + 60_000;
  while (true) {
    try {
      await access(path);
      return;
    } catch {
      if (Date.now() >= deadline) throw new Error(`timed out waiting for release marker ${path}`);
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
  }
}
