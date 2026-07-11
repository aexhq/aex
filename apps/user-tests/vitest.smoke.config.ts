import { defineConfig } from "vitest/config";
import { loadLocalEnv } from "./test/env-local";
import { PUBLISHED_ARTIFACT_SMOKE_FILES } from "./test/_fixtures/smoke-suite.js";
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
// It is deliberately not the complete release matrix. Platform deploy.yml
// installs this same sdk_version in dev + prd for every discovered gating SDK
// file and the isolated admission, heavy-session, and tool-fuzz lanes. Both
// planes must pass before promotion; this workflow owns only the fast registry
// artifact self-check.
const maxWorkers = workerCountFromEnv("AEX_USER_TEST_MAX_WORKERS", 2);

export default defineConfig({
  test: {
    environment: "node",
    globals: true,
    // Installing the packed SDK + CLI bundle can take a while on a cold runner.
    testTimeout: 180_000,
    hookTimeout: 180_000,
    include: Object.values(PUBLISHED_ARTIFACT_SMOKE_FILES),
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
