import { describe, expect, it } from "bun:test";
// @ts-expect-error JavaScript CI policy helper is validated directly.
import { classifyRetryableInstallFailure } from "../cicd/wait-for-bun-install.mjs";

const candidate = {
  packageName: "@aexhq/sdk",
  version: "0.42.0-canary",
};

describe("published artifact install retry policy", () => {
  it.each([
    ["target-version-not-visible", `404: No version matching ${candidate.packageName}@${candidate.version} was found`],
    ["registry-transport", "error: ECONNRESET while fetching registry.npmjs.org"],
    ["registry-rate-limit", "HTTP 429 Too Many Requests"],
    ["registry-server", "registry request failed with HTTP 503"],
    ["integrity-propagation", "package integrity checksum mismatch"],
  ])("retries only the named %s class", (expected, stderr) => {
    expect(classifyRetryableInstallFailure({ ...candidate, outputs: [stderr] })).toBe(expected);
  });

  it.each([
    "HTTP 401 unauthorized",
    "HTTP 403 forbidden",
    "No version matching broken-transitive-package@9.9.9 was found",
    "package requires an unsupported engine",
    "malformed package manifest",
  ])("fails fast on deterministic or unclassified errors: %s", (stderr) => {
    expect(classifyRetryableInstallFailure({ ...candidate, outputs: [stderr] })).toBeNull();
  });
});
