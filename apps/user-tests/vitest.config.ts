import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    // Installing a packed tarball with the SDK + CLI bundle can take a
    // while on a cold runner, especially on Windows. Keep this generous.
    testTimeout: 180_000,
    hookTimeout: 180_000,
    include: ["test/**/*.test.ts"],
    // The heavy full-feature long session is a SEPARATE explicit gate
    // (own `test:user:heavy` script + vitest.heavy.config.ts) that runs
    // only AFTER the rest of the user-tests pass. Keep it out of the
    // default `test:user` sweep so it never runs implicitly.
    exclude: ["**/node_modules/**", "test/live/live-sdk-heavy-session.test.ts"],
    // Each scenario spawns its own child processes (npm install, tsc,
    // node) with cwd in an install tempdir and drives a live run.
    // Unbounded parallelism multiplies disk usage and piles concurrent
    // live-run spend + managed runtime pressure, so cap at 3 files at once —
    // a bounded speedup over the previous fully-serial run. (The CI
    // fan-out already parallelizes e2e vs user vs heavy as separate jobs;
    // the heavy suite stays fully serial — see vitest.heavy.config.ts.)
    fileParallelism: true,
    maxWorkers: 3,
    minWorkers: 1,
    // Tests WITHIN a file run CONCURRENTLY. The shared install tempdir is
    // read-only after the `beforeAll` install, and every scenario writes a
    // UNIQUELY-named runner script (e.g. `outputs-<cell>.mjs`,
    // `user-envvars-native.mjs`, `comprehensive-native-anthropic.mjs`) with a
    // unique idempotencyKey — so concurrent tests in one file never collide on
    // disk or run identity. `maxConcurrency` bounds intra-file concurrency to 2;
    // with `maxWorkers: 3` that caps TOTAL concurrent live runs at ~6 (2x the
    // prior serial-within-file rate) — a deliberate bound on live-run spend +
    // Anthropic/DeepSeek rate limits + managed runtime pressure. Raise it to go
    // faster at higher spend/limit risk. (The heavy suite stays fully serial —
    // see vitest.heavy.config.ts.)
    maxConcurrency: 2,
    sequence: {
      concurrent: true
    }
  }
});
