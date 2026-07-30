import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { describe, expect, it } from "bun:test";
// @ts-expect-error JS release helper is validated directly.
import { applySdkVersion, buildCanaryVersion, isCanaryVersion } from "../cicd/canary-version.mjs";

const SHA_A = "ABCDEF0123456789abcdef0123456789abcdef01";
const SHA_B = "0123456789abcdef0123456789abcdef01234567";

describe("canary versioning", () => {
  it("carries the base version, the run id, and the source sha", () => {
    expect(buildCanaryVersion({ baseVersion: "0.41.5", sha: SHA_A, run: "19876543210" })).toBe(
      "0.41.5-canary.19876543210.gabcdef0"
    );
  });

  it("drops an existing prerelease before constructing the canary", () => {
    expect(buildCanaryVersion({ baseVersion: "1.2.3-next.4+build.2", sha: SHA_B, run: "7" })).toBe(
      "1.2.3-canary.7.g0123456"
    );
  });

  // THE REGRESSION. The old function validated the sha and then discarded it, so
  // every push at one base resolved the same string and npm refused the second
  // publish. Three published `0.4x.y-canary` versions on the registry are the
  // record of that being "fixed" by bumping the base, which buys one push.
  it("resolves two commits at one base version to two different versions", () => {
    const first = buildCanaryVersion({ baseVersion: "0.46.4", sha: SHA_A, run: "1001" });
    const second = buildCanaryVersion({ baseVersion: "0.46.4", sha: SHA_B, run: "1002" });

    expect(first).not.toBe(second);
    expect(first).toBe("0.46.4-canary.1001.gabcdef0");
    expect(second).toBe("0.46.4-canary.1002.g0123456");
  });

  it("orders canaries by run id, so the newest is also the semver-highest", () => {
    // Prerelease precedence compares dot-separated identifiers left to right and
    // numeric ones numerically. A bare `g<sha7>` suffix would sort lexically,
    // i.e. arbitrarily, and `npm view`/range resolution would pick a random
    // canary as the highest.
    const versions = ["9", "10", "100"].map((run) =>
      buildCanaryVersion({ baseVersion: "0.46.4", sha: SHA_A, run })
    );
    expect(versions).toEqual([
      "0.46.4-canary.9.gabcdef0",
      "0.46.4-canary.10.gabcdef0",
      "0.46.4-canary.100.gabcdef0"
    ]);
    expect(Bun.semver.order(versions[1]!, versions[0]!)).toBe(1);
    expect(Bun.semver.order(versions[2]!, versions[1]!)).toBe(1);
  });

  it("sorts above the bare `-canary` versions already on the registry", () => {
    // The three published `<base>-canary` versions stay valid npm versions; the
    // new scheme must not resolve below them or `npm publish` would still be
    // ordering the channel backwards.
    expect(Bun.semver.order(buildCanaryVersion({ baseVersion: "0.46.4", sha: SHA_A, run: "1" }), "0.46.4-canary")).toBe(1);
  });

  it("updates only the package version in a release checkout", () => {
    const root = join(tmpdir(), `aex-canary-version-${process.pid}-${Date.now()}`);
    const sdk = join(root, "packages", "sdk");
    mkdirSync(join(sdk, "src"), { recursive: true });
    writeFileSync(join(sdk, "package.json"), '{"name":"@aexhq/sdk","version":"0.1.0"}\n');

    applySdkVersion(root, "0.1.0-canary.42.gabcdef0");

    expect(JSON.parse(readFileSync(join(sdk, "package.json"), "utf8")).version).toBe(
      "0.1.0-canary.42.gabcdef0"
    );
  });

  it("refuses to apply a version that is not of the canary shape", () => {
    // Including the OLD shape: a caller still producing `<base>-canary` is a
    // caller that has not been migrated, and it must not reach npm.
    for (const version of ["0.1.0", "0.1.0-canary", "0.1.0-canary.gabcdef0", "0.1.0-canary.1.gABCDEF0"]) {
      expect(isCanaryVersion(version), version).toBe(false);
      expect(() => applySdkVersion("/does-not-matter", version), version).toThrow(/invalid canary version/);
    }
    expect(isCanaryVersion("0.1.0-canary.1.gabcdef0")).toBe(true);
  });

  it("rejects malformed inputs instead of producing a mutable or invalid tag", () => {
    expect(() => buildCanaryVersion({ baseVersion: "latest", sha: SHA_A, run: "1" })).toThrow(/base version/);
    expect(() => buildCanaryVersion({ baseVersion: "1.2.3", sha: "not-a-sha", run: "1" })).toThrow(/source SHA/);
    expect(() => buildCanaryVersion({ baseVersion: "1.2.3", sha: "abcdef012345", run: "1" })).toThrow(/source SHA/);
    // The run id is REQUIRED and never defaulted: a default would silently
    // collapse two commits back onto one version for any caller that forgot it.
    expect(() => buildCanaryVersion({ baseVersion: "1.2.3", sha: SHA_B })).toThrow(/run id/);
    expect(() => buildCanaryVersion({ baseVersion: "1.2.3", sha: SHA_B, run: "007" })).toThrow(/run id/);
    expect(() => buildCanaryVersion({ baseVersion: "1.2.3", sha: SHA_B, run: "abc" })).toThrow(/run id/);
  });
});
