import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";

loadLocalEnv();

/**
 * Dedicated config for the "heavy" full-feature long-session gate
 * (`bun run test:user:heavy`). Kept separate from vitest.config.ts so the
 * heavy suite runs ONLY when invoked explicitly — after the rest of the
 * live user-tests pass — never as part of the default `test:user` sweep
 * (which excludes this file). See test/live/live-sdk-heavy-session.test.ts.
 *
 * Timeouts are larger than the default config: each cell drives a
 * multi-minute session (skills + MCP + files + multi-step prompt +
 * files) and the per-test `it(...)` timeouts inside the file are the
 * real ceiling — these are the outer safety net.
 */
export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    testTimeout: 15 * 60_000,
    hookTimeout: 5 * 60_000,
    include: ["test/live/live-sdk-heavy-session.test.ts"],
    // Each cell spawns its own child process against the live hosted API and
    // costs real money; serialize so the three cells don't pile spend
    // and managed runtime pressure on at once.
    fileParallelism: false,
    sequence: {
      concurrent: false
    }
  }
});
