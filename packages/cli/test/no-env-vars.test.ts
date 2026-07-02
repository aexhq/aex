import { describe, expect, it } from "vitest";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const cliRoot = resolve(here, "..");
const bundlePath = resolve(cliRoot, "dist", "cli.mjs");
const digestPath = resolve(cliRoot, "dist", "cli.mjs.sha256");

/**
 * Mechanical lockdown of the shipped CLI bundle.
 *
 * The CLI MUST NOT read any `process.env.AEX_*` value at runtime.
 * Tests use module-DI through `internal.ts`; the shipped bundle has no
 * env-driven code paths.
 */
describe("CLI bundle: no env-var-driven code paths", () => {
  it("ships a self-contained ESM bundle at dist/cli.mjs", () => {
    expect(existsSync(bundlePath)).toBe(true);
    expect(existsSync(digestPath)).toBe(true);
  });

  it("uses the Bun runtime shebang", () => {
    const text = readFileSync(bundlePath, "utf8");
    expect(text.startsWith("#!/usr/bin/env bun\n")).toBe(true);
  });

  it("contains zero process.env.AEX_* references", () => {
    const text = readFileSync(bundlePath, "utf8");
    const matches = text.match(/process\.env\.AEX_[A-Z0-9_]+/g) ?? [];
    expect(matches).toEqual([]);
  });

  it("never reads the historic test-only override env vars", () => {
    // These are explicitly named in the plan as forbidden surfaces.
    const forbidden = [
      "AEX_CLI_BUNDLE_PATH"
    ];
    const text = readFileSync(bundlePath, "utf8");
    for (const name of forbidden) {
      expect(text.includes(name)).toBe(false);
    }
  });

  it("references the fixed in-container manifest path exactly", () => {
    const text = readFileSync(bundlePath, "utf8");
    expect(text.includes("/mnt/session/uploads/aex/index.json")).toBe(true);
  });
});
