import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";
import { workerCountFromEnv } from "./vitest.worker-count";

loadLocalEnv();

const maxWorkers = workerCountFromEnv("AEX_USER_TEST_OFFLINE_MAX_WORKERS", 4);

export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    // Installing and type-checking the packed SDK can take a while on a cold
    // runner, especially on Windows. Keep this generous.
    testTimeout: 180_000,
    hookTimeout: 180_000,
    include: ["test/offline/**/*.test.ts"],
    exclude: ["**/node_modules/**"],
    // Offline files are independent once the artifact tarball is selected.
    // The package script pre-packs one tarball for local/CI sessions, so parallel
    // file slots spend time testing instead of each packing the SDK in sequence.
    fileParallelism: true,
    maxWorkers,
    minWorkers: 1,
    // Keep tests inside one file serial: several files write fixed helper
    // scripts into their per-file install tree.
    maxConcurrency: 1,
    sequence: {
      concurrent: false
    }
  }
});
