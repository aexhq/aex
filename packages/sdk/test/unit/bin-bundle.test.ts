import { describe, expect, it } from "vitest";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(here, "..", "..");
const sdkPkg = JSON.parse(readFileSync(resolve(sdkRoot, "package.json"), "utf8")) as {
  readonly bin?: Record<string, string>;
};
const sdkBundlePath = resolve(sdkRoot, sdkPkg.bin?.aex ?? "");
const cliBundlePath = resolve(sdkRoot, "..", "cli", "dist", "cli.mjs");

/**
 * Lock the agent-first surface invariant on the SHIPPED artifact level.
 *
 * The published `@aexhq/sdk` package must expose exactly one binary
 * — `aex` — pointing at a real ESM bundle inside its own dist
 * directory. If `bin` ever goes missing, the hosted runtime mounts no CLI in
 * the session container and the entire agent-first surface decision
 * collapses; the user-test suite catches this from the outside, but
 * this unit test catches it before publish.
 */
describe("aex package: CLI bin surface", () => {
  it("declares exactly one bin entry named `aex`", () => {
    const bin = sdkPkg.bin ?? {};
    expect(Object.keys(bin)).toEqual(["aex"]);
    expect(bin.aex).toBe("./dist/cli.mjs");
  });

  it("ships a bundled CLI in dist after bun build", () => {
    // The SDK's build script copies @aexhq/cli's bundle into its own
    // dist. If you see this fail locally, run
    // `bun run --cwd ../.. --filter @aexhq/sdk build` first.
    expect(existsSync(cliBundlePath)).toBe(true);
    expect(existsSync(sdkBundlePath)).toBe(true);
  });

  it("bundles a byte-identical copy of the @aexhq/cli artifact", () => {
    // No silent skip: a missing bundle must fail loudly (run
    // `bun run --cwd ../.. --filter @aexhq/sdk build` first). The presence test above
    // documents the same build hint on its own failure path.
    expect(existsSync(sdkBundlePath)).toBe(true);
    expect(existsSync(cliBundlePath)).toBe(true);
    const sdkText = readFileSync(sdkBundlePath);
    const cliText = readFileSync(cliBundlePath);
    expect(sdkText.equals(cliText)).toBe(true);
  });
});
