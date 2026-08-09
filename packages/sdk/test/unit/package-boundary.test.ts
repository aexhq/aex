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

test("no source module imports a package, so the contract table cannot arrive as a dependency", () => {
  // The route table is generated INTO `src/generated/` rather than imported from
  // `@aexhq/wire` precisely so the published package stays dependency-free. An
  // empty `dependencies` map does not catch a bare import on its own: bun and a
  // workspace install would both resolve it locally and only a consumer would
  // see the break.
  for (const file of walk(resolve(packageRoot, "src"))) {
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(/\bfrom\s+"([^"]+)"/g)) {
      expect({ file, specifier: match[1], relative: match[1]?.startsWith(".") })
        .toEqual({ file, specifier: match[1], relative: true });
    }
  }
});

function walk(directory: string): string[] {
  return readdirSync(directory).flatMap((entry) => {
    const path = resolve(directory, entry);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}
