import { mkdir, readdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

const root = process.argv[2];
const markerDir = join(root, "active");
await mkdir(markerDir, { recursive: true });
const marker = join(markerDir, String(process.pid));
await writeFile(marker, "active", "utf8");
try {
  await new Promise((resolve) => setTimeout(resolve, 100));
  if ((await readdir(markerDir)).length > 1) {
    await writeFile(join(root, `overlap-${process.pid}`), "overlap", "utf8");
  }
} finally {
  await rm(marker, { force: true });
}
