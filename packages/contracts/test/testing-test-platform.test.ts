import { describe, expect, it, spyOn } from "bun:test";
import { caps, testPlatformCapabilities } from "../src/testing.js";

const CAPABILITY_NAMES = [
  "canSymlinkFiles",
  "canSymlinkDirectories",
  "canJunction",
  "posixFileModes",
  "spawnedPidsVisible",
  "windowsDrivePaths"
] as const;

describe("test-platform capabilities", () => {
  it("resolves every capability to a boolean exactly once, logging the resolution", () => {
    const log = spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const first = testPlatformCapabilities();
      const second = testPlatformCapabilities();
      expect(second).toBe(first);
      for (const name of CAPABILITY_NAMES) {
        expect(typeof first[name]).toBe("boolean");
      }
      const resolutionLogs = log.mock.calls.filter(([line]) =>
        String(line).includes("[test-platform]")
      );
      expect(resolutionLogs).toHaveLength(1);
      expect(String(resolutionLogs[0]![0])).toContain("canSymlinkFiles=");
    } finally {
      log.mockRestore();
    }
  });

  it("exposes the same resolved values through the caps view", () => {
    const resolved = testPlatformCapabilities();
    for (const name of CAPABILITY_NAMES) {
      expect(caps[name]).toBe(resolved[name]);
    }
  });

  it("pins the statically-known capabilities to the current OS", () => {
    const isWindows = process.platform === "win32";
    expect(caps.windowsDrivePaths).toBe(isWindows);
    expect(caps.spawnedPidsVisible).toBe(!isWindows);
    if (!isWindows) {
      expect(caps.canJunction).toBe(false);
      expect(caps.posixFileModes).toBe(true);
    } else {
      expect(caps.posixFileModes).toBe(false);
    }
  });

  it("returns a frozen capability record", () => {
    expect(Object.isFrozen(testPlatformCapabilities())).toBe(true);
  });
});
