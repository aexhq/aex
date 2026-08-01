import { expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";

const packageRoot = resolve(import.meta.dir, "../..");

test("the SDK has no runtime dependencies or node specifiers", () => {
  const manifest = JSON.parse(readFileSync(resolve(packageRoot, "package.json"), "utf8")) as {
    dependencies?: Record<string, string>;
    type?: string;
  };
  expect(manifest.dependencies).toEqual({});
  expect(manifest.type).toBe("module");

  for (const file of walk(resolve(packageRoot, "src"))) {
    expect(readFileSync(file, "utf8")).not.toContain('from "node:');
    expect(readFileSync(file, "utf8")).not.toContain("from 'node:");
  }
});

function walk(directory: string): string[] {
  return readdirSync(directory).flatMap((entry) => {
    const path = resolve(directory, entry);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}
