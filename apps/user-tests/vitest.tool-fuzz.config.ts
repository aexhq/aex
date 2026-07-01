import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";

loadLocalEnv();

/**
 * Dedicated paid live gate for seeded builtin/custom tool capability fuzzing.
 * It is separate from the default user sweep so deploy pipelines and manual
 * reproductions can invoke it directly, while missing live credentials still
 * fail closed.
 */
export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    testTimeout: 20 * 60_000,
    hookTimeout: 5 * 60_000,
    include: ["test/live/live-sdk-tool-capability-fuzz.test.ts"],
    fileParallelism: false,
    maxConcurrency: 1,
    retry: 1,
    sequence: { concurrent: false }
  }
});
