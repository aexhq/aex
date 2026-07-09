import { describe, expect, it } from "vitest";
// @ts-expect-error JS helper is validated directly.
import { analyzeFeatureGate } from "../cicd/feature-gate.mjs";

describe("feature gate impact mapper", () => {
  it("fans public contract changes out to every public feature-done gate", () => {
    const impact = analyzeFeatureGate(["packages/contracts/src/events.ts"]);

    expect(impact.gates).toMatchObject({
      lint: true,
      unit_tests: true,
      offline_user_tests: true,
      docs_build: true,
      pack_sdk: true
    });
  });

  it("keeps docs-only changes off package/user-test gates", () => {
    const impact = analyzeFeatureGate(["packages/sdk/docs/sessions.md"]);

    expect(impact.gates).toMatchObject({
      lint: true,
      docs_build: true,
      unit_tests: false,
      offline_user_tests: false,
      pack_sdk: false
    });
  });

  it("treats workflow and release script edits as automation-sensitive", () => {
    const impact = analyzeFeatureGate([".github/workflows/release.yml", "scripts/cicd/release-manifest.mjs"]);

    expect(impact.gates.lint).toBe(true);
    expect(impact.gates.unit_tests).toBe(true);
    expect(impact.gates.offline_user_tests).toBe(true);
    expect(impact.gates.pack_sdk).toBe(true);
  });
});
