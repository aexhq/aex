import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  unlinkSync,
  writeFileSync
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const runnerRelativePath = "scripts/cicd/run-validation-tests.mjs";
const configRelativePath = "scripts/validate/vitest.config.ts";
const runnerPath = resolve(repoRoot, runnerRelativePath);
const configPath = resolve(repoRoot, configRelativePath);

describe("repository validation Vitest isolation", () => {
  it("uses its own config when a polluted self-hosted parent has a hostile config", { timeout: 20_000 }, () => {
    const manifest = JSON.parse(readFileSync(resolve(repoRoot, "package.json"), "utf8")) as {
      readonly scripts?: Record<string, string>;
    };
    expect(manifest.scripts?.["test:validate"]).toBe(
      `bun scripts/with-generated-dist-lock.mjs bun ${runnerRelativePath}`
    );
    expect(manifest.scripts?.["test:unit"]).toContain("bun run test:validate");
    expect(existsSync(runnerPath)).toBe(true);
    expect(existsSync(configPath)).toBe(true);

    const hostRoot = mkdtempSync(join(tmpdir(), "aex-hostile-vitest-parent-"));
    const checkoutRoot = resolve(hostRoot, "aex");
    const nodeModulesLink = resolve(checkoutRoot, "node_modules");
    try {
      const fixtureRunner = resolve(checkoutRoot, runnerRelativePath);
      const fixtureConfig = resolve(checkoutRoot, configRelativePath);
      mkdirSync(dirname(fixtureRunner), { recursive: true });
      mkdirSync(dirname(fixtureConfig), { recursive: true });
      copyFileSync(runnerPath, fixtureRunner);
      copyFileSync(configPath, fixtureConfig);
      writeFileSync(
        resolve(hostRoot, "vitest.config.ts"),
        `throw new Error("HOSTILE_ANCESTOR_CONFIG_LOADED");\n`,
        "utf8"
      );
      writeFileSync(
        resolve(checkoutRoot, "package.json"),
        `${JSON.stringify({ name: "aex-vitest-isolation-fixture", private: true, type: "module" }, null, 2)}\n`,
        "utf8"
      );
      writeFileSync(
        resolve(checkoutRoot, "scripts/validate/probe.test.ts"),
        [
          `import { expect, it } from "vitest";`,
          `it("runs with the checkout-owned config", () => expect(true).toBe(true));`,
          ""
        ].join("\n"),
        "utf8"
      );
      symlinkSync(
        resolve(repoRoot, "node_modules"),
        nodeModulesLink,
        process.platform === "win32" ? "junction" : "dir"
      );

      const result = spawnSync("bun", [fixtureRunner], {
        cwd: checkoutRoot,
        encoding: "utf8",
        env: { ...process.env, NO_COLOR: "1" },
        timeout: 20_000
      });
      const output = `${result.stdout ?? ""}${result.stderr ?? ""}`.replaceAll("\\", "/");
      const normalizedCheckout = checkoutRoot.replaceAll("\\", "/");

      expect(result.status, output).toBe(0);
      expect(output).toContain("validation-test-context:");
      expect(output).toContain(`cwd=${normalizedCheckout}`);
      expect(output).toContain(`root=${normalizedCheckout}/scripts/validate`);
      expect(output).toContain(`config=${normalizedCheckout}/scripts/validate/vitest.config.ts`);
      expect(output).not.toContain("HOSTILE_ANCESTOR_CONFIG_LOADED");
      expect(output).toContain("1 passed");
    } finally {
      if (existsSync(nodeModulesLink)) unlinkSync(nodeModulesLink);
      rmSync(hostRoot, { recursive: true, force: true });
    }
  });

  it("keeps the runner and config inside the public repository", () => {
    for (const path of [runnerPath, configPath]) {
      const rel = relative(repoRoot, path).replaceAll("\\", "/");
      expect(rel).not.toMatch(/^\.\.(?:\/|$)/);
    }
  });
});
