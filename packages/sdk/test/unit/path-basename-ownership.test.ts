import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(here, "..", "..");

function source(name: string): string {
  return readFileSync(resolve(sdkRoot, "src", `${name}.ts`), "utf8");
}

describe("SDK composition basename ownership", () => {
  it("keeps one private cross-platform basename normalizer shared by File and Skill", () => {
    const helper = source("path-basename");
    const file = source("file");
    const skill = source("skill");
    const normalisationChain = String.raw`.replace(/\\/g, "/").replace(/\/+$/, "")`;

    expect(helper).toContain(normalisationChain);
    expect(file).not.toContain(normalisationChain);
    expect(skill).not.toContain(normalisationChain);
    expect(file).toContain('from "./path-basename.js"');
    expect(skill).toContain('from "./path-basename.js"');
    expect(file.match(/crossPlatformBasename\(/g)).toHaveLength(2);
    expect(skill.match(/crossPlatformBasename\(/g)).toHaveLength(1);
  });

  it("keeps File and Skill slug grammars domain-owned and the helper unpublished", () => {
    const file = source("file");
    const skill = source("skill");
    const index = source("index");
    const packageJson = JSON.parse(readFileSync(resolve(sdkRoot, "package.json"), "utf8")) as {
      readonly exports: Readonly<Record<string, unknown>>;
    };

    expect(file).toContain("function slugFromFilename(");
    expect(file).toContain("DERIVED_FILE_STORAGE_SLUG_PATTERN");
    expect(skill).toContain("deriveSkillName(source, front.name, explicitName, dirBasename)");
    expect(skill).not.toContain("slugFromFilename");
    expect(index).not.toContain("crossPlatformBasename");
    expect(Object.keys(packageJson.exports)).toEqual(["."]);
  });
});
