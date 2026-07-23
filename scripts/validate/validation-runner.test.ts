import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const runnerRelativePath = "scripts/cicd/run-validation-tests.mjs";
const gateRelativePath = "scripts/cicd/assert-no-skips.mjs";
const junitHelperRelativePath = "scripts/cicd/junit-report.mjs";
const runnerPath = resolve(repoRoot, runnerRelativePath);

// Successor of the retired vitest-config-isolation lock: the validation
// runner must execute the CHECKOUT-OWNED scripts/validate tree under `bun
// test`, immune to any configuration a polluted self-hosted parent directory
// carries. bun reads bunfig.toml from the invocation cwd only — the runner
// pins that cwd inside the checkout, and this fixture proves a hostile
// ancestor bunfig (throwing preload) and a hostile ancestor test file are
// never loaded.
describe("repository validation runner isolation", () => {
  it("runs the checkout-owned validate tree despite a hostile parent bunfig", () => {
    const manifest = JSON.parse(readFileSync(resolve(repoRoot, "package.json"), "utf8")) as {
      readonly scripts?: Record<string, string>;
    };
    expect(manifest.scripts?.["test:validate"]).toBe(
      `bun scripts/with-generated-dist-lock.mjs bun ${runnerRelativePath}`
    );
    expect(manifest.scripts?.["test:unit"]).toContain("bun run test:validate");
    expect(existsSync(runnerPath)).toBe(true);

    const hostRoot = mkdtempSync(join(tmpdir(), "aex-hostile-bun-parent-"));
    const checkoutRoot = resolve(hostRoot, "aex");
    try {
      for (const file of [runnerRelativePath, gateRelativePath, junitHelperRelativePath]) {
        const target = resolve(checkoutRoot, file);
        mkdirSync(dirname(target), { recursive: true });
        copyFileSync(resolve(repoRoot, file), target);
      }
      // Hostile parent: a bunfig whose preload throws, plus a failing test
      // file. Neither may ever be picked up by the checkout-pinned run.
      writeFileSync(
        resolve(hostRoot, "bunfig.toml"),
        `[test]\npreload = ["./hostile-preload.ts"]\n`,
        "utf8"
      );
      writeFileSync(
        resolve(hostRoot, "hostile-preload.ts"),
        `throw new Error("HOSTILE_ANCESTOR_CONFIG_LOADED");\n`,
        "utf8"
      );
      writeFileSync(
        resolve(hostRoot, "hostile.test.ts"),
        [
          `import { it } from "bun:test";`,
          `it("hostile parent test must never be collected", () => {`,
          `  throw new Error("HOSTILE_ANCESTOR_TEST_COLLECTED");`,
          `});`,
          ""
        ].join("\n"),
        "utf8"
      );
      writeFileSync(
        resolve(checkoutRoot, "package.json"),
        `${JSON.stringify({ name: "aex-bun-isolation-fixture", private: true, type: "module" }, null, 2)}\n`,
        "utf8"
      );
      mkdirSync(resolve(checkoutRoot, "scripts/validate"), { recursive: true });
      writeFileSync(
        resolve(checkoutRoot, "scripts/validate/probe.test.ts"),
        [
          `import { expect, it } from "bun:test";`,
          `it("runs from the checkout-owned validate tree", () => expect(true).toBe(true));`,
          ""
        ].join("\n"),
        "utf8"
      );

      const result = spawnSync(
        "bun" in process.versions ? process.execPath : "bun",
        [resolve(checkoutRoot, runnerRelativePath)],
        {
          cwd: checkoutRoot,
          encoding: "utf8",
          env: { ...process.env, NO_COLOR: "1" },
          timeout: 30_000
        }
      );
      const output = `${result.stdout ?? ""}${result.stderr ?? ""}`.replaceAll("\\", "/");
      const normalizedCheckout = checkoutRoot.replaceAll("\\", "/");

      expect(result.status, output).toBe(0);
      expect(output).toContain("validation-test-context:");
      expect(output).toContain(`cwd=${normalizedCheckout}`);
      expect(output).toContain(`root=${normalizedCheckout}/scripts/validate`);
      expect(output).toContain("runner=bun-test");
      expect(output).not.toContain("HOSTILE_ANCESTOR_CONFIG_LOADED");
      expect(output).not.toContain("HOSTILE_ANCESTOR_TEST_COLLECTED");
      expect(output).toContain("1 pass");
      // The junit + no-skips release gate ran against the checkout's report.
      expect(output).toMatch(/\[assert-no-skips\] OK: 1 tests/);
      expect(existsSync(resolve(checkoutRoot, ".tmp/junit-test-validate.xml"))).toBe(true);
    } finally {
      rmSync(hostRoot, { recursive: true, force: true });
    }
  }, 30_000);

  it("keeps the runner and its gate inside the public repository", () => {
    for (const path of [runnerPath, resolve(repoRoot, gateRelativePath), resolve(repoRoot, junitHelperRelativePath)]) {
      const rel = relative(repoRoot, path).replaceAll("\\", "/");
      expect(rel).not.toMatch(/^\.\.(?:\/|$)/);
    }
  });
});
