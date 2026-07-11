import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";
import { workerCountFromEnv } from "./vitest.worker-count";

loadLocalEnv();

const maxWorkers = workerCountFromEnv("AEX_USER_TEST_MAX_WORKERS", 2);

export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    // Installing a packed tarball with the SDK + CLI bundle can take a
    // while on a cold runner, especially on Windows. Keep this generous.
    testTimeout: 180_000,
    hookTimeout: 180_000,
    include: ["test/**/*.test.ts"],
    // Some suites are SEPARATE explicit gates kept out of the default
    // `test:user` sweep so they never run implicitly:
    //   - the cap-saturating admission-gate suite (own isolated low-cap
    //     workspace lane in platform aws-suite.yml);
    //   - the heavy full-feature long session (own `test:user:heavy` +
    //     vitest.heavy.config.ts), run only AFTER the rest pass;
    //   - the per-provider correctness round-trips under test/live/providers/
    //     (own `test:user:providers` + vitest.providers.config.ts), run only
    //     on demand so the extra-provider matrix never piles spend on push.
    exclude: [
      "**/node_modules/**",
      "test/live/edge-admission-gates.user.test.ts",
      "test/live/live-sdk-heavy-session.test.ts",
      "test/live/live-api-fuzz.test.ts",
      "test/live/live-sdk-tool-capability-fuzz.test.ts",
      "test/live/providers/**"
    ],
    // Each scenario spawns its own child processes (bun install, tsc,
    // bun) with cwd in an install tempdir and drives a live run.
    // Unbounded parallelism multiplies disk usage and piles concurrent
    // live-run spend + managed runtime pressure, so default local runs cap at
    // 2 files at once. CI can raise AEX_USER_TEST_MAX_WORKERS after selecting
    // one artifact tarball/version for all parallel file slots.
    fileParallelism: true,
    maxWorkers,
    minWorkers: 1,
    // Tests WITHIN a file run CONCURRENTLY. The shared install tempdir is
    // read-only after the `beforeAll` install, and every scenario writes a
    // UNIQUELY-named runner script (e.g. `files-<cell>.mjs`,
    // `user-envvars-managed.mjs`, `comprehensive-managed-anthropic.mjs`) with a
    // unique idempotencyKey — so concurrent tests in one file never collide on
    // disk or session identity. `maxConcurrency: 1` serializes live cells inside a
    // file; combined with maxWorkers, that is the deliberate bound on live-run
    // spend, provider rate limits, and managed runtime pressure. Raise it to go
    // faster at higher spend/limit risk. (The heavy suite stays fully serial —
    // see vitest.heavy.config.ts.)
    maxConcurrency: 1,
    sequence: {
      concurrent: true
    }
  }
});
