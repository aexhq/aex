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
 * The CLI MUST NOT read any `process.env.ANTPATH_*` value at runtime.
 * Earlier drafts proposed test-only overrides like
 * `ANTPATH_RUN_TOKEN_FILE` and `ANTPATH_PROXY_BASE_FILE` — those are a
 * prompt-injection attack surface (a compromised agent could set them
 * to redirect the bearer mint). Tests use module-DI through
 * `internal.ts`; the shipped bundle has no env-driven code paths.
 */
describe("CLI bundle: no env-var-driven code paths", () => {
  it("ships a self-contained ESM bundle at dist/cli.mjs", () => {
    expect(existsSync(bundlePath)).toBe(true);
    expect(existsSync(digestPath)).toBe(true);
  });

  it("contains zero process.env.ANTPATH_* references", () => {
    const text = readFileSync(bundlePath, "utf8");
    const matches = text.match(/process\.env\.ANTPATH_[A-Z0-9_]+/g) ?? [];
    expect(matches).toEqual([]);
  });

  it("never reads the historic test-only override env vars", () => {
    // These are explicitly named in the plan as forbidden surfaces.
    const forbidden = [
      "ANTPATH_RUN_TOKEN_FILE",
      "ANTPATH_PROXY_BASE_FILE",
      "ANTPATH_PROXY_INDEX_FILE",
      "ANTPATH_CLI_BUNDLE_PATH"
    ];
    const text = readFileSync(bundlePath, "utf8");
    for (const name of forbidden) {
      expect(text.includes(name)).toBe(false);
    }
  });

  it("references the fixed in-container paths exactly", () => {
    // The CLI's discoverability surface is the manifest + run-token
    // mounted under Anthropic's `/mnt/session/uploads/<mount_path>`
    // rebase. Asserting their presence in the bundle catches any
    // refactor that accidentally relativizes them or reverts to the
    // pre-mount-fix `/antpath/...` paths (which the Anthropic
    // managed runner environment forwarding ignores these — see proxy-bootstrap.ts
    // for the gory details).
    const text = readFileSync(bundlePath, "utf8");
    expect(text.includes("/mnt/session/uploads/antpath/index.json")).toBe(true);
    expect(text.includes("/mnt/session/uploads/antpath/run-token")).toBe(true);
  });
});
