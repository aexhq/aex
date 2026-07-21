import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const source = readFileSync(new URL("../src/asset-bundle.ts", import.meta.url), "utf8");

function functionBody(name: string, nextName: string): string {
  return source.slice(source.indexOf(`export function ${name}(`), source.indexOf(nextName));
}

describe("canonical asset bundle pipeline ownership", () => {
  it("keeps entry collection and archive finalization in one shared pipeline", () => {
    const pipeline = source.slice(
      source.indexOf("function bundleCanonicalFiles<"),
      source.indexOf("export function bundleSkillFiles(")
    );

    expect(source.match(/typeof contents === "string" \? TEXT\.encode\(contents\) : contents/g)).toHaveLength(1);
    expect(source.match(/totalDecompressed \+= bytes\.byteLength/g)).toHaveLength(1);
    expect(source.match(/zipSync\(zippable, \{ level: 6 \}\)/g)).toHaveLength(1);
    expect(source.match(/fileCount: collected\.size/g)).toHaveLength(1);
    expect(pipeline.match(/contains duplicate path:/g)).toHaveLength(1);
  });

  it("keeps the skill and tool exports as policy-only adapters", () => {
    const skill = functionBody("bundleSkillFiles", "export interface BundledTool");
    const tool = functionBody("bundleToolFiles", "const ZIP_EPOCH");

    expect(skill).toContain("bundleCanonicalFiles(");
    expect(tool).toContain("bundleCanonicalFiles(");
    expect(skill).not.toContain("for (const [rawPath, contents]");
    expect(tool).not.toContain("for (const [rawPath, contents]");
    expect(skill).not.toContain("zipSync(");
    expect(tool).not.toContain("zipSync(");
  });
});
