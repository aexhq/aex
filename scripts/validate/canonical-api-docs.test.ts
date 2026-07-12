import { readdirSync, readFileSync, statSync } from "node:fs";
import { posix, relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const repoRoot = resolve(import.meta.dirname, "..", "..");
const publicDocs = [
  "README.md",
  "packages/sdk/README.md",
  "packages/sdk/docs",
  "apps/docs/content/docs",
  "examples"
] as const;

const bareRoute = /\b(?:GET|POST|PUT|PATCH|DELETE) \/(?!api(?:\/|\b))(?:sessions|assets|secrets|whoami|workspace|billing|webhook|mcp-servers)\b/;
const legacyRootResource = /\b(?:aex|client)\.(?:files|messages|secrets)\./;

describe("canonical public API documentation", () => {
  it("uses /api routes and the hierarchical SDK resource surface", () => {
    const findings: string[] = [];
    for (const entry of publicDocs) {
      for (const file of filesUnder(resolve(repoRoot, entry))) {
        readFileSync(file, "utf8").split(/\r?\n/).forEach((line, index) => {
          if (bareRoute.test(line) || legacyRootResource.test(line)) {
            findings.push(`${relative(repoRoot, file).replaceAll("\\", "/")}:${index + 1}: ${line.trim()}`);
          }
        });
      }
    }
    expect(findings).toEqual([]);
  });

  it("teaches File.mountPath as a directory rather than a destination filename", () => {
    const quickstart = readFileSync(resolve(repoRoot, "packages/sdk/docs/quickstart.md"), "utf8");
    const files = readFileSync(resolve(repoRoot, "packages/sdk/docs/files.md"), "utf8");
    const example = /File\.fromPath\("([^"]+)",\s*\{\s*mountPath:\s*"([^"]+)"/.exec(quickstart);
    const sourceName = posix.basename(example?.[1] ?? "");
    const mountDirectory = example?.[2] ?? "";
    const resolvedPath = `${mountDirectory.replace(/\/$/, "")}/${sourceName}`;

    expect(sourceName).not.toBe("");
    expect(posix.basename(mountDirectory)).not.toBe(sourceName);
    expect(files).toContain(`\`${resolvedPath}\``);
    expect(files).toContain("`mountPath` is always a destination directory");
    expect(files).toContain("storage slug; it never renames the mounted file");
  });
});

function filesUnder(path: string): string[] {
  if (statSync(path).isFile()) return [path];
  return readdirSync(path, { withFileTypes: true }).flatMap((entry) => {
    const child = resolve(path, entry.name);
    if (entry.isDirectory()) return filesUnder(child);
    return entry.isFile() && /\.(?:md|ts|mjs)$/.test(entry.name) ? [child] : [];
  });
}
