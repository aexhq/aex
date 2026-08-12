import { describe, expect, test } from "bun:test";

import { DASHBOARD_BUILD_STEPS } from "../cicd/build-dashboard-output.mjs";

describe("dashboard build output", () => {
  test("builds both generated TypeScript dependencies before Next consumes them", () => {
    expect(DASHBOARD_BUILD_STEPS).toEqual([
      ["run", "--filter", "@aexhq/wire", "build"],
      ["run", "--filter", "@aexhq/sdk", "build"],
      ["run", "vercel", "build", "--standalone", "--no-color"],
    ]);
  });
});
