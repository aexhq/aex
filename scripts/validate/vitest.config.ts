import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  root: dirname(fileURLToPath(import.meta.url)),
  test: {
    environment: "node",
    // Repository validators scan Git state and spawn nested package-manager,
    // compiler, and test processes. Serial files keep those probes from
    // starving one another on shared and high-core runners.
    fileParallelism: false
  }
});
