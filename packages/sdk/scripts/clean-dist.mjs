import { rm } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(here, "..");

await rm(resolve(sdkRoot, "dist"), { recursive: true, force: true });
