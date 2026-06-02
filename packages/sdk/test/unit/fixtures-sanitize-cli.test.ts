import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

/**
 * End-to-end CLI contract for the REAL `sanitize-api-fixtures.ts` script
 * (resolving the package.json `fixtures:sanitize` ref): it must EXIT NON-ZERO
 * when a committed `*.sanitized.json` still contains a secret-shaped value, and
 * zero when clean. We point it at a scratch dir via ANTPATH_FIXTURES_DIR so no
 * secret is ever planted in the real fixtures tree.
 */

const SCRIPT = fileURLToPath(new URL("../../scripts/sanitize-api-fixtures.ts", import.meta.url));
// These tests synchronously spawn `npx tsx <script>`; an `npx`/tsx cold-start
// under the parallel unit-suite load can exceed vitest's 5s default. Give the
// subprocess room so the suite is not flaky (the script itself runs in ~1s).
const CLI_TIMEOUT_MS = 30_000;
// A synthetic value matching the value-agnostic sk-ant shape (not a real key).
const SK_ANT = "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF0123";

let dir: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "antpath-fixtures-"));
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

/** Run `tsx sanitize-api-fixtures.ts --verify` against `dir`; return exit code.
 * `shell: true` so the `npx` launcher resolves on Windows (.cmd) too. */
function runVerify(): number {
  try {
    execFileSync("npx", ["tsx", SCRIPT, "--verify"], {
      stdio: "pipe",
      shell: true,
      env: { ...process.env, ANTPATH_FIXTURES_DIR: dir }
    });
    return 0;
  } catch (err) {
    return (err as { status?: number }).status ?? 1;
  }
}

describe("fixtures:sanitize CLI gate (exit code contract)", () => {
  it("exits 0 when every committed fixture is clean", () => {
    writeFileSync(
      join(dir, "clean.sanitized.json"),
      JSON.stringify({
        version: 1,
        scenario: "clean",
        source: "derived-seed",
        events: [{ id: "<id>", type: "session.status_idle", stop_reason: "end_turn" }]
      }),
      "utf8"
    );
    expect(runVerify()).toBe(0);
  }, CLI_TIMEOUT_MS);

  it("exits NON-ZERO when a committed fixture still carries a secret", () => {
    writeFileSync(
      join(dir, "leaky.sanitized.json"),
      JSON.stringify({
        version: 1,
        scenario: "leaky",
        source: "live-capture",
        events: [{ id: "<id>", type: "agent.message", leaked: SK_ANT }]
      }),
      "utf8"
    );
    expect(runVerify()).toBe(1);
  }, CLI_TIMEOUT_MS);
});
