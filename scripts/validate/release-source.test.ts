import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { describe, expect, it } from "bun:test";
// @ts-expect-error JS release helper is validated directly.
import { applyReleaseSource } from "../cicd/release-source.mjs";

describe("release source attestation", () => {
  it("writes the exact release commit into the package registry metadata", () => {
    const root = join(tmpdir(), `aex-release-source-${process.pid}-${Date.now()}`);
    const sdk = join(root, "packages", "sdk");
    mkdirSync(sdk, { recursive: true });
    writeFileSync(join(sdk, "package.json"), '{"name":"@aexhq/sdk","version":"0.42.0"}\n');

    const sha = "ABCDEF0123456789abcdef0123456789abcdef01";
    applyReleaseSource(root, sha);

    expect(JSON.parse(readFileSync(join(sdk, "package.json"), "utf8"))).toMatchObject({
      name: "@aexhq/sdk",
      version: "0.42.0",
      aexRelease: {
        sourceSha: sha.toLowerCase()
      }
    });
  });

  it("rejects partial SHAs and unexpected packages", () => {
    const root = join(tmpdir(), `aex-release-source-invalid-${process.pid}-${Date.now()}`);
    const sdk = join(root, "packages", "sdk");
    mkdirSync(sdk, { recursive: true });
    writeFileSync(join(sdk, "package.json"), '{"name":"not-aex","version":"0.42.0"}\n');

    expect(() => applyReleaseSource(root, "a".repeat(12))).toThrow(/source SHA/);
    expect(() => applyReleaseSource(root, "a".repeat(40))).toThrow(/unexpected package/);
  });
});
