import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";

loadLocalEnv();

/**
 * Dedicated config for the LIVE adversarial-input fuzz of the dev API
 * (`bun run test:user:fuzz`). Kept SEPARATE from vitest.config.ts so the fuzz
 * sweep runs ONLY when invoked explicitly — never as part of the default
 * `test:user` sweep (which excludes this file). It fails fast unless
 * AEX_API_URL + AEX_API_TOKEN are set, so invoking this script is a hard gate.
 *
 * The suite uses RAW fetch (no SDK, no child process / packed-tarball install),
 * and bounds fast-check numRuns so the whole sweep stays in minutes. It only
 * ever sends submit bodies that are edge-rejected before any run dispatch, so it
 * costs no real run spend.
 */
export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    testTimeout: 5 * 60_000,
    hookTimeout: 60_000,
    include: ["test/live/live-api-fuzz.test.ts"],
    fileParallelism: false,
    sequence: { concurrent: false }
  }
});
