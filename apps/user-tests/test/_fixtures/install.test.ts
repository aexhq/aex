import { describe, expect, it } from "bun:test";
import { resolveArtifactSpecs } from "./install.js";

describe("clean-install artifact selection", () => {
  it("requires SDK and CLI tarballs as one tested pair", async () => {
    await expect(resolveArtifactSpecs(
      { AEX_USER_TEST_SDK_TARBALL: "sdk.tgz" },
      async () => { throw new Error("local pack must not run"); }
    )).rejects.toThrow(/both AEX_USER_TEST_SDK_TARBALL and AEX_USER_TEST_CLI_TARBALL/);
  });

  it("requires exact SDK and CLI registry versions as one tested pair", async () => {
    expect(await resolveArtifactSpecs(
      {
        AEX_USER_TEST_SDK_VERSION: "0.46.5",
        AEX_USER_TEST_CLI_VERSION: "0.33.0"
      },
      async () => { throw new Error("local pack must not run"); }
    )).toEqual({
      sdk: "@aexhq/sdk@0.46.5",
      cli: "@aexhq/cli@0.33.0",
      source: "registry"
    });
  });

  it("packs the current workspace when no artifact selector is supplied", async () => {
    const expected = { sdk: "sdk.tgz", cli: "cli.tgz", source: "local-pack" as const };
    expect(await resolveArtifactSpecs({}, async () => expected)).toEqual(expected);
  });
});
