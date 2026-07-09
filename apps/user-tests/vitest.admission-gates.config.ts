import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";

loadLocalEnv();

/**
 * Dedicated config for the cap-saturating admission-gate suite
 * (`bun run test:user:admission-gates`). The default `test:user` sweep excludes
 * this file so it cannot starve unrelated live tests; CI sessions it only with an
 * isolated low-cap workspace.
 */
export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    testTimeout: 10 * 60_000,
    hookTimeout: 5 * 60_000,
    include: ["test/live/edge-admission-gates.user.test.ts"],
    exclude: ["**/node_modules/**"],
    fileParallelism: false,
    sequence: {
      concurrent: false
    }
  }
});
