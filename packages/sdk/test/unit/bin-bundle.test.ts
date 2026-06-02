import { describe, expect, it } from "vitest";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(here, "..", "..");
const sdkPkg = JSON.parse(readFileSync(resolve(sdkRoot, "package.json"), "utf8")) as {
  readonly bin?: Record<string, string>;
};
const sdkBundlePath = resolve(sdkRoot, sdkPkg.bin?.antpath ?? "");
const cliBundlePath = resolve(sdkRoot, "..", "cli", "dist", "cli.mjs");

/**
 * Lock the agent-first surface invariant on the SHIPPED artifact level.
 *
 * The published `antpath` npm package must expose exactly one binary
 * — `antpath` — pointing at a real ESM bundle inside its own dist
 * directory. If `bin` ever goes missing, the worker mounts no CLI in
 * the run container and the entire agent-first surface decision
 * collapses; the user-test suite catches this from the outside, but
 * this unit test catches it before publish.
 */
describe("antpath package: CLI bin surface", () => {
  it("declares exactly one bin entry named `antpath`", () => {
    const bin = sdkPkg.bin ?? {};
    expect(Object.keys(bin)).toEqual(["antpath"]);
    expect(bin.antpath).toBe("./dist/cli.mjs");
  });

  it("ships a bundled CLI in dist after pnpm build", () => {
    // The SDK's build script copies @antpath/cli's bundle into its own
    // dist. If you see this fail locally, run `pnpm --filter antpath
    // run build` (or the workspace `pnpm build`) first.
    expect(existsSync(cliBundlePath)).toBe(true);
    expect(existsSync(sdkBundlePath)).toBe(true);
  });

  it("bundles a byte-identical copy of the @antpath/cli artifact", () => {
    if (!existsSync(sdkBundlePath) || !existsSync(cliBundlePath)) return;
    const sdkText = readFileSync(sdkBundlePath);
    const cliText = readFileSync(cliBundlePath);
    expect(sdkText.equals(cliText)).toBe(true);
  });
});
