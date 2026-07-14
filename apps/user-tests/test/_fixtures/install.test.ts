import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { createDirectoryCleanup, getBunCommand, resolveInstallSpec, runCommand } from "./install.js";

describe("user-test install artifact resolution", () => {
  it("uses AEX_USER_TEST_TARBALL when set", async () => {
    const spec = await resolveInstallSpec({
      env: { AEX_USER_TEST_TARBALL: "/tmp/aexhq-sdk-0.0.0.tgz" },
      pathExists: () => true,
      packLocalSdk: async () => {
        throw new Error("local pack must not run for explicit tarball");
      }
    });

    expect(spec).toEqual({ spec: "/tmp/aexhq-sdk-0.0.0.tgz", source: "tarball" });
  });

  it("uses AEX_USER_TEST_VERSION when set", async () => {
    const spec = await resolveInstallSpec({
      env: { AEX_USER_TEST_VERSION: "1.2.3" },
      packLocalSdk: async () => {
        throw new Error("local pack must not run for explicit version");
      }
    });

    expect(spec).toEqual({ spec: "@aexhq/sdk@1.2.3", source: "registry" });
  });

  it("rejects when tarball and version are both set", async () => {
    await expect(
      resolveInstallSpec({
        env: {
          AEX_USER_TEST_TARBALL: "/tmp/aexhq-sdk-0.0.0.tgz",
          AEX_USER_TEST_VERSION: "1.2.3"
        }
      })
    ).rejects.toThrow(/mutually exclusive/);
  });

  it("rejects a missing explicit tarball", async () => {
    await expect(
      resolveInstallSpec({
        env: { AEX_USER_TEST_TARBALL: "/tmp/missing-aexhq-sdk.tgz" },
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
        return "/tmp/aex-local/aexhq-sdk-1.2.3.tgz";
      }
    });

    expect(packCalls).toBe(1);
    expect(spec).toEqual({
      spec: "/tmp/aex-local/aexhq-sdk-1.2.3.tgz",
      source: "local-pack"
    });
  });

  it("includes partial stdout and stderr when a child command times out", async () => {
    const dir = mkdtempSync(join(tmpdir(), "aex-session-command-timeout-"));
    try {
      const script = join(dir, "timeout-diagnostics.mjs");
      writeFileSync(
        script,
        [
          'process.stdout.write("stdout-before-timeout\\n");',
          'process.stderr.write("stderr-before-timeout\\n");',
          "await new Promise((resolve) => setTimeout(resolve, 30_000));"
        ].join("\n")
      );

      await expect(runCommand(getBunCommand(), [script], { timeoutMs: 1500 })).rejects.toThrow(
        /stdout-before-timeout[\s\S]*stderr-before-timeout/
      );
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("removes install trees idempotently", () => {
    const dir = mkdtempSync(join(tmpdir(), "aex-install-cleanup-test-"));
    writeFileSync(join(dir, "hardlinked-package-file"), "fixture");

    const cleanup = createDirectoryCleanup(dir);
    cleanup();
    cleanup();

    expect(existsSync(dir)).toBe(false);
  });

  it("fails closed with actionable diagnostics when an install tree cannot be removed", () => {
    const dir = join(tmpdir(), "aex-install-cleanup-locked");
    let removeCalls = 0;
    const cleanup = createDirectoryCleanup(dir, {
      pathExists: () => true,
      removeDirectory: () => {
        removeCalls++;
        throw Object.assign(new Error("file is in use"), {
          code: "EPERM",
          syscall: "unlink",
          path: join(dir, "node_modules", "ignore", "LICENSE-MIT")
        });
      }
    });

    expect(cleanup).toThrow(/failed to remove install tempdir[\s\S]*EPERM[\s\S]*LICENSE-MIT/);
    expect(cleanup).toThrow(/failed to remove install tempdir/);
    expect(removeCalls).toBe(2);
  });
});
