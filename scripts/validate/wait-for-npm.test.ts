import { describe, expect, it, vi } from "vitest";
// @ts-expect-error JavaScript CI policy helper is validated directly.
import { isRetryableRegistryStatus, validateRegistryMetadata, waitForNpmEvidence } from "../cicd/wait-for-npm.mjs";

const expected = {
  packageName: "@aexhq/sdk",
  version: "0.42.0-canary.123.g0123456789ab",
  sourceSha: "0123456789abcdef0123456789abcdef01234567"
};

const metadata = {
  name: expected.packageName,
  version: expected.version,
  aexRelease: { sourceSha: expected.sourceSha },
  dist: {
    integrity: "sha512-YWJjZA==",
    tarball: "https://registry.npmjs.org/@aexhq/sdk/-/sdk.tgz"
  }
};

describe("immutable npm evidence", () => {
  it("carries one successful metadata observation through tarball visibility", async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce({ ok: true, status: 200, json: async () => metadata })
      .mockResolvedValueOnce({ ok: true, status: 200 });

    await expect(waitForNpmEvidence(expected, { fetch })).resolves.toMatchObject({
      attempt: 1,
      integrity: metadata.dist.integrity,
      sourceSha: expected.sourceSha,
      tarballStatus: 200
    });
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("returns integrity and source identity from the same registry observation", () => {
    expect(validateRegistryMetadata(metadata, expected)).toEqual({
      integrity: metadata.dist.integrity,
      sourceSha: expected.sourceSha,
      tarballUrl: metadata.dist.tarball
    });
  });

  it("fails closed when the registry observation is not source-bound", () => {
    expect(() =>
      validateRegistryMetadata(
        { ...metadata, aexRelease: { sourceSha: "fedcba9876543210fedcba9876543210fedcba98" } },
        expected
      )
    ).toThrow("does not match release source");
  });

  it.each([404, 408, 425, 429, 500, 502, 503, 504])("retries propagation status %i", (status) => {
    expect(isRetryableRegistryStatus(status)).toBe(true);
  });

  it.each([200, 400, 401, 403, 409, 422])("does not retry deterministic status %i", (status) => {
    expect(isRetryableRegistryStatus(status)).toBe(false);
  });
});
