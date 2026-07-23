import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { describe, expect, it } from "bun:test";
// @ts-expect-error JS release helper is validated directly.
import { applySdkVersion, buildCanaryVersion } from "../cicd/canary-version.mjs";

describe("canary versioning", () => {
  it("derives the canary semver from the package version and validates the source SHA", () => {
    expect(
      buildCanaryVersion({
        baseVersion: "0.41.5",
        sha: "ABCDEF0123456789abcdef0123456789abcdef01"
      })
    ).toBe("0.41.5-canary");
  });

  it("drops an existing prerelease before constructing the canary", () => {
    expect(
      buildCanaryVersion({
        baseVersion: "1.2.3-next.4+build.2",
        sha: "0123456789abcdef0123456789abcdef01234567"
      })
    ).toBe("1.2.3-canary");
  });

  it("updates only the package version and exported SDK version in a release checkout", () => {
    const root = join(tmpdir(), `aex-canary-version-${process.pid}-${Date.now()}`);
    const sdk = join(root, "packages", "sdk");
    mkdirSync(join(sdk, "src"), { recursive: true });
    writeFileSync(join(sdk, "package.json"), '{"name":"@aexhq/sdk","version":"0.1.0"}\n');
    writeFileSync(join(sdk, "src", "version.ts"), 'export const SDK_VERSION = "0.1.0";\n');

    applySdkVersion(root, "0.1.0-canary");

    expect(JSON.parse(readFileSync(join(sdk, "package.json"), "utf8")).version).toBe(
      "0.1.0-canary"
    );
    expect(readFileSync(join(sdk, "src", "version.ts"), "utf8")).toContain(
      'SDK_VERSION = "0.1.0-canary"'
    );
  });

  it("rejects malformed inputs instead of producing a mutable or invalid tag", () => {
    expect(() => buildCanaryVersion({ baseVersion: "latest", sha: "abcdef0123456789abcdef0123456789abcdef01" })).toThrow(
      /base version/
    );
    expect(() => buildCanaryVersion({ baseVersion: "1.2.3", sha: "not-a-sha" })).toThrow(
      /source SHA/
    );
    expect(() => buildCanaryVersion({ baseVersion: "1.2.3", sha: "abcdef012345" })).toThrow(/source SHA/);
  });
});
