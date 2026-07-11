import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";
import { workerCountFromEnv } from "./vitest.worker-count";

loadLocalEnv();

// The release-time PUBLISHED-ARTIFACT SMOKE gate.
//
// release.yml publishes the SDK to a pre-release dist-tag, then runs this to
// prove the PUBLISHED @aexhq/sdk + CLI installs from the registry and works
// end-to-end — a DeepSeek SDK round-trip on the managed runtime and the
// installed CLI. Both files `bun install @aexhq/sdk@<version>` in their own
// tempdir (AEX_USER_TEST_VERSION is set), so this exercises the real npm
// artifact, not a local pack.
//
// It is DELIBERATELY not the full behavioral matrix (test/**). That runs
// against this same published sdk_version across dev + prd in platform
// deploy.yml (aws-suite.yml `sdk_user_tests`), which the release workflow attemptbook (§4.7)
// requires green before promoting to `latest`. The release only needs this fast
// published-artifact self-check; retrying the whole suite here was redundant
// with that deploy gate.
const maxWorkers = workerCountFromEnv("AEX_USER_TEST_MAX_WORKERS", 2);

export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    // Installing the packed SDK + CLI bundle can take a while on a cold runner.
    testTimeout: 180_000,
    hookTimeout: 180_000,
    include: ["test/live/live-sdk-deepseek.test.ts", "test/live/live-cli-installed.test.ts"],
    exclude: ["**/node_modules/**"],
    fileParallelism: true,
    maxWorkers,
    minWorkers: 1,
    maxConcurrency: 1,
    sequence: {
      concurrent: true
    }
  }
});
