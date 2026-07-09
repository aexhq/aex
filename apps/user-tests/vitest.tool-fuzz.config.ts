import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";
import { workerCountFromEnv } from "./vitest.worker-count";

loadLocalEnv();

/**
 * Dedicated paid live gate for seeded builtin/custom tool capability fuzzing.
 * It is separate from the default user sweep so deploy pipelines and manual
 * reproductions can invoke it directly, while missing live credentials still
 * fail closed.
 *
 * This is ONE file, so vitest file-parallelism can't split it — the only
 * parallelism lever is running the seeded cells CONCURRENTLY within the file.
 * That is safe: after the single `beforeAll` install the tree is read-only,
 * and every scenario drops a UNIQUELY-named runner script with a UNIQUE
 * idempotency key against an independent live run, so concurrent cells never
 * collide on disk or session identity. `maxConcurrency` is therefore the deliberate
 * bound on concurrent live-run spend and provider rate limits; raise
 * AEX_USER_TEST_TOOL_FUZZ_CONCURRENCY to go faster at higher spend/limit risk.
 */
const maxConcurrency = workerCountFromEnv("AEX_USER_TEST_TOOL_FUZZ_CONCURRENCY", 4);

export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    testTimeout: 20 * 60_000,
    hookTimeout: 5 * 60_000,
    include: ["test/live/live-sdk-tool-capability-fuzz.test.ts"],
    fileParallelism: false,
    maxConcurrency,
    retry: 1,
    sequence: { concurrent: true }
  }
});
