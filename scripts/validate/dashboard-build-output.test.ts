import { describe, expect, test } from "bun:test";

import { readRepoFile } from "./workflow-test-helpers.js";

const BUILD_SCRIPT = "scripts/cicd/build-dashboard-output.mjs";

describe("dashboard build output", () => {
  test("builds both generated TypeScript dependencies before Next consumes them", () => {
    const source = readRepoFile(BUILD_SCRIPT);
    const wireBuild = source.indexOf(
      'run(["run", "--filter", "@aexhq/wire", "build"]);'
    );
    const sdkBuild = source.indexOf(
      'run(["run", "--filter", "@aexhq/sdk", "build"]);'
    );
    const vercelBuild = source.indexOf(
      'run(["run", "vercel", "build", "--no-color"]);'
    );

    expect(wireBuild).toBeGreaterThan(-1);
    expect(sdkBuild).toBeGreaterThan(wireBuild);
    expect(vercelBuild).toBeGreaterThan(sdkBuild);
  });
});
