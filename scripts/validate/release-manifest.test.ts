import { describe, expect, it } from "vitest";
// @ts-expect-error JS helper is validated directly.
import { buildPublicReleaseManifest, validatePlatformValidationManifest, validatePublicReleaseManifest } from "../cicd/release-manifest.mjs";

describe("release manifest contract", () => {
  it("writes a first-class public candidate manifest", () => {
    const manifest = buildPublicReleaseManifest({
      version: "0.40.17",
      distTag: "canary",
      runId: "123",
      runAttempt: "1",
      headSha: "abc",
      createdAt: "2026-07-09T00:00:00.000Z"
    });

    expect(manifest).toMatchObject({
      schemaVersion: 1,
      kind: "aex-public-release-manifest",
      workflow: "release.yml",
      runId: "123",
      sdk: { packageName: "@aexhq/sdk", version: "0.40.17", initialDistTag: "canary" },
      promotion: { status: "candidate" }
    });
    expect(manifest.gates).toEqual(
      expect.arrayContaining(["publish", "published-artifact-smoke", "live-user-tests-preflight"])
    );
    expect(validatePublicReleaseManifest(manifest, { version: "0.40.17", runId: "123" })).toEqual({
      ok: true,
      errors: []
    });
  });

  it("rejects public manifests for the wrong repo, package, dist-tag, or head", () => {
    const manifest = buildPublicReleaseManifest({
      version: "0.40.17",
      distTag: "canary",
      runId: "123",
      headSha: "abc"
    });

    expect(validatePublicReleaseManifest({ ...manifest, repository: "aexhq/aex-platform" }).errors).toContain(
      "repository must be aexhq/aex"
    );
    expect(validatePublicReleaseManifest({ ...manifest, headSha: "" }).errors).toContain("headSha is required");
    expect(validatePublicReleaseManifest({ ...manifest, sdk: { ...manifest.sdk, packageName: "@aexhq/other" } }).errors).toContain(
      "sdk.packageName must be @aexhq/sdk"
    );
    expect(validatePublicReleaseManifest({ ...manifest, sdk: { ...manifest.sdk, initialDistTag: "" } }).errors).toContain(
      "sdk.initialDistTag is required"
    );
  });

  it("fails platform validation manifests that do not prove every release gate", () => {
    const result = validatePlatformValidationManifest(
      {
        schemaVersion: 1,
        kind: "aex-platform-validation-manifest",
        platform: { runId: "456" },
        publicRelease: { runId: "123" },
        sdk: { version: "0.40.17" },
        gates: ["suite_dev"],
        images: { brain: { tag: "sha-a" }, egress: { tag: "sha-b" }, byok: { tag: "sha-c" } }
      },
      { version: "0.40.17", runId: "456", publicReleaseRunId: "123" }
    );

    expect(result.ok).toBe(false);
    expect(result.errors).toEqual(
      expect.arrayContaining(["missing platform gate spot_canary_dev", "missing platform gate suite_prod", "missing platform gate smoke_prod"])
    );
  });
});
