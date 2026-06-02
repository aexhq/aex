import { describe, expect, it } from "vitest";
import { resolveInstallSpec } from "./install.js";

describe("user-test install artifact resolution", () => {
  it("uses ANTPATH_USER_TEST_TARBALL when set", async () => {
    const spec = await resolveInstallSpec({
      env: { ANTPATH_USER_TEST_TARBALL: "/tmp/antpath-0.0.0.tgz" },
      pathExists: () => true,
      packLocalSdk: async () => {
        throw new Error("local pack must not run for explicit tarball");
      }
    });

    expect(spec).toEqual({ spec: "/tmp/antpath-0.0.0.tgz", source: "tarball" });
  });

  it("uses ANTPATH_USER_TEST_VERSION when set", async () => {
    const spec = await resolveInstallSpec({
      env: { ANTPATH_USER_TEST_VERSION: "0.13.3" },
      packLocalSdk: async () => {
        throw new Error("local pack must not run for explicit version");
      }
    });

    expect(spec).toEqual({ spec: "antpath@0.13.3", source: "registry" });
  });

  it("rejects when tarball and version are both set", async () => {
    await expect(
      resolveInstallSpec({
        env: {
          ANTPATH_USER_TEST_TARBALL: "/tmp/antpath-0.0.0.tgz",
          ANTPATH_USER_TEST_VERSION: "0.13.3"
        }
      })
    ).rejects.toThrow(/mutually exclusive/);
  });

  it("rejects a missing explicit tarball", async () => {
    await expect(
      resolveInstallSpec({
        env: { ANTPATH_USER_TEST_TARBALL: "/tmp/missing-antpath.tgz" },
        pathExists: () => false
      })
    ).rejects.toThrow(/non-existent path/);
  });

  it("defaults to a local packed SDK tarball when no explicit artifact is set", async () => {
    let packCalls = 0;
    const spec = await resolveInstallSpec({
      env: {},
      packLocalSdk: async () => {
        packCalls++;
        return "/tmp/antpath-local/antpath-0.13.3.tgz";
      }
    });

    expect(packCalls).toBe(1);
    expect(spec).toEqual({
      spec: "/tmp/antpath-local/antpath-0.13.3.tgz",
      source: "local-pack"
    });
  });
});
