import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";

// Load `.env.local` (AEX_TEST_ACCOUNT_TOKEN / AEX_API_URL / AEX_E2E_CLI_ENTRY)
// before test collection so the suite's fail-closed requireEnv checks resolve.
loadLocalEnv();

// LIVE black-box CLI control-plane e2e (WS6). Kept OUT of every other config's
// glob and out of the default `test:user` sweep — reachable only via
// `test:user:e2e`. Serial + single worker: the flows mutate one provisioned
// account's control plane, so parallelism would race org/workspace/key state.
export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    testTimeout: 120_000,
    hookTimeout: 120_000,
    include: ["test/e2e/**/*.test.ts"],
    exclude: ["**/node_modules/**"],
    fileParallelism: false,
    maxWorkers: 1,
    minWorkers: 1,
    maxConcurrency: 1,
    sequence: {
      concurrent: false
    }
  }
});
