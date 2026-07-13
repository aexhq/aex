import { describe, expect, it } from "vitest";
// @ts-expect-error JS helper is validated directly.
import { buildPublicReleaseManifest, validatePlatformValidationManifest, validatePublicReleaseManifest } from "../cicd/release-manifest.mjs";

const PUBLIC_SHA = "a".repeat(40);
const OTHER_PUBLIC_SHA = "b".repeat(40);
const PLATFORM_SHA = "c".repeat(40);
const SDK_INTEGRITY = "sha512-dGVzdA==";
const OTHER_SDK_INTEGRITY = "sha512-b3RoZXI=";
const BRAIN_DIGEST = `sha256:${"1".repeat(64)}`;
const EGRESS_DIGEST = `sha256:${"2".repeat(64)}`;
const BYOK_DIGEST = `sha256:${"3".repeat(64)}`;

function imageEvidence() {
  return {
    brain: {
      tag: `sha-${PLATFORM_SHA.slice(0, 12)}-pub-${PUBLIC_SHA.slice(0, 12)}`,
      dev: { repository: "aex-dev-eu-west-2-brain", digest: BRAIN_DIGEST },
      prd: { repository: "aex-prd-eu-west-2-brain", digest: BRAIN_DIGEST }
    },
    egress: {
      tag: `sha-${"d".repeat(12)}`,
      dev: { repository: "aex-dev-eu-west-2-egress-proxy", digest: EGRESS_DIGEST },
      prd: { repository: "aex-prd-eu-west-2-egress-proxy", digest: EGRESS_DIGEST }
    },
    byok: {
      tag: `sha-${"e".repeat(12)}`,
      dev: { repository: "aex-dev-eu-west-2-byok-inject", digest: BYOK_DIGEST },
      prd: { repository: "aex-prd-eu-west-2-byok-inject", digest: BYOK_DIGEST }
    }
  };
}

describe("release manifest contract", () => {
  it("writes a first-class public candidate manifest", () => {
    const manifest = buildPublicReleaseManifest({
      repository: "aexhq/aex",
      version: "0.40.17",
      distTag: "canary",
      runId: "123",
      runAttempt: "1",
      headSha: PUBLIC_SHA,
      integrity: SDK_INTEGRITY,
      createdAt: "2026-07-09T00:00:00.000Z"
    });

    expect(manifest).toMatchObject({
      schemaVersion: 3,
      kind: "aex-public-release-manifest",
      workflow: "release.yml",
      runId: "123",
      sdk: {
        packageName: "@aexhq/sdk",
        version: "0.40.17",
        integrity: SDK_INTEGRITY,
        initialDistTag: "canary"
      },
      cli: { packageName: "@aexhq/sdk", version: "0.40.17", bin: "aex", integrity: SDK_INTEGRITY },
      promotion: { status: "candidate" }
    });
    expect(manifest.gates).toEqual(
      expect.arrayContaining(["publish", "published-artifact-smoke", "live-user-tests-preflight"])
    );
    expect(validatePublicReleaseManifest(manifest, {
      version: "0.40.17",
      runId: "123",
      headSha: PUBLIC_SHA,
      integrity: SDK_INTEGRITY
    })).toEqual({
      ok: true,
      errors: []
    });
  });

  it("rejects public manifests for the wrong repo, package, dist-tag, or head", () => {
    const manifest = buildPublicReleaseManifest({
      repository: "aexhq/aex",
      version: "0.40.17",
      distTag: "canary",
      runId: "123",
      headSha: PUBLIC_SHA,
      integrity: SDK_INTEGRITY
    });

    expect(validatePublicReleaseManifest({ ...manifest, repository: "aexhq/platform" }).errors).toContain(
      "repository must be aexhq/aex"
    );
    expect(validatePublicReleaseManifest({ ...manifest, headSha: "" }).errors).toContain("headSha is required");
    expect(validatePublicReleaseManifest({ ...manifest, sdk: { ...manifest.sdk, packageName: "@aexhq/other" } }).errors).toContain(
      "sdk.packageName must be @aexhq/sdk"
    );
    expect(validatePublicReleaseManifest({ ...manifest, sdk: { ...manifest.sdk, initialDistTag: "" } }).errors).toContain(
      "sdk.initialDistTag is required"
    );
    expect(validatePublicReleaseManifest({ ...manifest, sdk: { ...manifest.sdk, integrity: "" } }).errors).toContain(
      "sdk.integrity is required"
    );
    expect(validatePublicReleaseManifest({ ...manifest, cli: { ...manifest.cli, version: "0.40.18" } }).errors).toContain(
      "cli.version must match sdk.version"
    );
  });

  it("rejects public manifests whose source SHA or integrity differs from the promoted candidate", () => {
    const manifest = buildPublicReleaseManifest({
      repository: "aexhq/aex",
      version: "0.40.17",
      distTag: "canary",
      runId: "123",
      headSha: PUBLIC_SHA,
      integrity: SDK_INTEGRITY
    });

    expect(validatePublicReleaseManifest(manifest, { headSha: OTHER_PUBLIC_SHA }).errors).toContain(
      `headSha must be ${OTHER_PUBLIC_SHA}`
    );
    expect(validatePublicReleaseManifest(manifest, { integrity: OTHER_SDK_INTEGRITY }).errors).toContain(
      `sdk.integrity must be ${OTHER_SDK_INTEGRITY}`
    );
  });

  it("fails platform validation manifests that do not prove every release gate", () => {
    const result = validatePlatformValidationManifest(
      {
        schemaVersion: 4,
        kind: "platform-validation-manifest",
        platform: { runId: "456", headSha: PLATFORM_SHA },
        publicRelease: { runId: "123", headSha: PUBLIC_SHA },
        sdk: { version: "0.40.17", integrity: SDK_INTEGRITY },
        gates: ["suite_dev"],
        images: imageEvidence()
      },
      {
        version: "0.40.17",
        runId: "456",
        publicReleaseRunId: "123",
        publicReleaseHeadSha: PUBLIC_SHA,
        integrity: SDK_INTEGRITY
      }
    );

    expect(result.ok).toBe(false);
    expect(result.errors).toEqual(
      expect.arrayContaining(["missing platform gate spot_canary_dev", "missing platform gate suite_prod", "missing platform gate smoke_prod"])
    );
  });

  it("rejects platform evidence bound to a different public SHA or package integrity", () => {
    const manifest = {
      schemaVersion: 4,
      kind: "platform-validation-manifest",
      platform: { runId: "456", headSha: PLATFORM_SHA },
      publicRelease: { runId: "123", headSha: PUBLIC_SHA },
      sdk: { version: "0.40.17", integrity: SDK_INTEGRITY },
      gates: ["suite_dev", "spot_canary_dev", "suite_prod", "smoke_prod"],
      images: imageEvidence()
    };

    expect(validatePlatformValidationManifest(manifest, { publicReleaseHeadSha: OTHER_PUBLIC_SHA }).errors).toContain(
      `publicRelease.headSha must be ${OTHER_PUBLIC_SHA}`
    );
    expect(validatePlatformValidationManifest(manifest, { integrity: OTHER_SDK_INTEGRITY }).errors).toContain(
      `sdk.integrity must be ${OTHER_SDK_INTEGRITY}`
    );
  });

  it("rejects platform proof whose prd image digest differs from the dev-tested artifact", () => {
    const images = imageEvidence();
    images.brain.prd.digest = `sha256:${"9".repeat(64)}`;
    const result = validatePlatformValidationManifest({
      schemaVersion: 4,
      kind: "platform-validation-manifest",
      platform: { runId: "456", headSha: PLATFORM_SHA },
      publicRelease: { runId: "123", headSha: PUBLIC_SHA },
      sdk: { version: "0.40.17", integrity: SDK_INTEGRITY },
      gates: ["suite_dev", "spot_canary_dev", "suite_prod", "smoke_prod"],
      images
    });

    expect(result.errors).toContain("images.brain prd digest must match the dev-tested digest");
  });
});
