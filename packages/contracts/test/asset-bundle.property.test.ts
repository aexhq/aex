import fc from "fast-check";
import { unzipSync } from "fflate";
import { describe, expect, it } from "bun:test";
import {
  bundleSkillFiles,
  bundleToolFiles,
  type SkillFiles,
  type ToolBundleManifest
} from "../src/internal.js";

const TEXT = new TextEncoder();
const SKILL_MD = "---\nname: property\ndescription: Property bundle\n---\n";
const TOOL_MANIFEST: ToolBundleManifest = {
  name: "property_tool",
  description: "Property bundle",
  input_schema: { type: "object", properties: {} },
  entry: "entry.mjs"
};

const contentArbitrary = fc.oneof(
  fc.string({ maxLength: 128 }),
  fc.uint8Array({ maxLength: 128 })
);

function bytes(contents: string | Uint8Array): Uint8Array {
  return typeof contents === "string" ? TEXT.encode(contents) : contents;
}

describe("canonical asset bundle property", () => {
  it("keeps arbitrary caller files byte-stable across skill and tool insertion orders", () => {
    fc.assert(fc.property(
      fc.array(contentArbitrary, { maxLength: 12 }),
      (contents) => {
        const common = contents.map((content, index) => [`files/f${index}.bin`, content] as const);
        const skillEntries = [["SKILL.md", SKILL_MD] as const, ...common];
        const toolEntries = [["entry.mjs", "export default () => null;\n"] as const, ...common];
        const skillFiles = Object.fromEntries(skillEntries) as SkillFiles;
        const reverseSkillFiles = Object.fromEntries([...skillEntries].reverse()) as SkillFiles;
        const toolFiles = Object.fromEntries(toolEntries) as SkillFiles;
        const reverseToolFiles = Object.fromEntries([...toolEntries].reverse()) as SkillFiles;

        const skill = bundleSkillFiles(skillFiles);
        const tool = bundleToolFiles(toolFiles, TOOL_MANIFEST);
        expect(skill.zip).toEqual(bundleSkillFiles(reverseSkillFiles).zip);
        expect(tool.zip).toEqual(bundleToolFiles(reverseToolFiles, TOOL_MANIFEST).zip);
        expect(skill.fileCount).toBe(skillEntries.length);
        expect(tool.fileCount).toBe(toolEntries.length + 1);

        const skillArchive = unzipSync(skill.zip);
        const toolArchive = unzipSync(tool.zip);
        for (const [path, content] of common) {
          // expect<...>: fflate types entries as Uint8Array<ArrayBuffer>; bytes()
          // returns the default ArrayBufferLike flavour. Same runtime shape.
          expect<Uint8Array | undefined>(skillArchive[path]).toEqual(bytes(content));
          expect<Uint8Array | undefined>(toolArchive[path]).toEqual(bytes(content));
        }
      }
    ), { numRuns: 100 });
  });
});
