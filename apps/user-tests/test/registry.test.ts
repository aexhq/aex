import { expect, test } from "bun:test";
import { existsSync } from "node:fs";
import { resolve } from "node:path";

import { resolveArtifactSelection } from "../artifacts.js";
import { USER_SCENARIOS } from "../scenarios.js";

test("the typed scenario registry has exactly 47 unique entries across six suites", () => {
  expect(USER_SCENARIOS).toHaveLength(47);
  expect(new Set(USER_SCENARIOS.map(({ id }) => id))).toHaveProperty("size", 47);
  expect(new Set(USER_SCENARIOS.map(({ suite }) => suite))).toEqual(
    new Set(["packed", "local", "live", "browser", "money", "operator"]),
  );
  for (const scenario of USER_SCENARIOS) {
    expect(scenario.id).toMatch(/^(packed|local|live|browser|money|operator)\.[a-z0-9]+(?:-[a-z0-9]+)*$/);
    expect(scenario.estimatedSeconds).toBeGreaterThan(0);
    expect(existsSync(resolve(import.meta.dir, "..", scenario.file))).toBe(true);
  }
});

test("packed artifact selection is paired, exact, and mutually exclusive", () => {
  expect(resolveArtifactSelection({})).toEqual({ kind: "workspace" });
  expect(() => resolveArtifactSelection({ AEX_USER_TEST_SDK_TARBALL: "sdk.tgz" })).toThrow();
  expect(() => resolveArtifactSelection({
    AEX_USER_TEST_SDK_VERSION: "^0.50.0", AEX_USER_TEST_CLI_VERSION: "0.50.0",
  })).toThrow();
  expect(resolveArtifactSelection({
    AEX_USER_TEST_SDK_VERSION: "0.50.0", AEX_USER_TEST_CLI_VERSION: "0.50.0",
  })).toEqual({ kind: "versions", sdkVersion: "0.50.0", cliVersion: "0.50.0" });
});
