import { expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");

test("the clean-cut client workspace contains only the supported surfaces", () => {
  const cargo = readFileSync(resolve(root, "Cargo.toml"), "utf8");
  for (const member of [
    "tools/aex-cli",
    "tests/live/aex-live-dashboard",
    "tests/live/aex-live-site",
  ]) {
    expect(cargo).toContain(`\"${member}\"`);
  }

  for (const retired of [
    "packages/cli",
    "packages/contracts",
    "apps/docs",
    "packages/sdk/docs",
    "scripts/docs",
    "scripts/openapi",
  ]) {
    expect(existsSync(resolve(root, retired))).toBe(false);
  }

  const manifest = JSON.parse(
    readFileSync(resolve(root, "package.json"), "utf8"),
  ) as { scripts: Record<string, string> };
  for (const [name, command] of Object.entries(manifest.scripts)) {
    expect(name).not.toMatch(/^(contracts|openapi|docs):/);
    expect(command).not.toMatch(/packages\/(?:contracts|cli)|apps\/docs|scripts\/(?:docs|openapi)/);
  }
});
