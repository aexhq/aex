import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import { CANONICAL_SHA256_DIGEST_PATTERN } from "../src/canonical-sha256.js";
import { RESERVED_META_ENTRY, SKILL_BUNDLE_LIMITS } from "../src/index.js";
import {
  bundleSkillFiles,
  bundleToolFiles,
  hashSkillBundle,
  normalizeToolManifest,
  type SkillFiles,
  type ToolBundleManifest
} from "../src/internal.js";

const TOOL_MANIFEST: ToolBundleManifest = {
  name: "owner_tool",
  description: "Owner tool",
  input_schema: { type: "object", properties: {} },
  entry: "nested/tool.mjs"
};

function apparentBytes(byteLength: number): Uint8Array {
  return new Proxy(new Uint8Array(0), {
    get(target, property) {
      if (property === "byteLength") return byteLength;
      return Reflect.get(target, property, target) as unknown;
    }
  });
}

describe("canonical internal asset bundle owner", () => {
  it("hashes deterministic bundles with the wire-format digest", async () => {
    const first = bundleSkillFiles({
      "z.txt": "last",
      "SKILL.md": "---\nname: owner\ndescription: Owner\n---\n",
      "a.txt": "first"
    });
    const second = bundleSkillFiles({
      "a.txt": "first",
      "SKILL.md": "---\nname: owner\ndescription: Owner\n---\n",
      "z.txt": "last"
    });

    expect(first.zip).toEqual(second.zip);
    expect(CANONICAL_SHA256_DIGEST_PATTERN.test(await hashSkillBundle(first.zip))).toBe(true);
  });

  it("rejects reserved metadata paths and regular-file leaf-prefix conflicts", () => {
    expect(() => bundleSkillFiles({
      "SKILL.md": "---\nname: owner\ndescription: Owner\n---\n",
      [RESERVED_META_ENTRY]: "caller-owned"
    })).toThrow(/reserved .*\.aexmeta\.json/);

    expect(() => bundleSkillFiles({
      "SKILL.md": "---\nname: owner\ndescription: Owner\n---\n",
      nested: "leaf",
      "nested/file.txt": "descendant"
    })).toThrow(/leaf prefix conflict/);
  });

  it("uses one normalized manifest for canonical tool archives", () => {
    const files = { "nested/tool.mjs": "export default async () => ({ ok: true });\n" };
    const manifest = normalizeToolManifest("Tool.fromFiles", {
      name: "owner_tool",
      description: "Owner tool",
      input_schema: { type: "object", properties: {} },
      entry: "nested/tool.mjs"
    }, files);

    expect(bundleToolFiles(files, manifest).zip.byteLength).toBeGreaterThan(0);
    expect(manifest.entry).toBe("nested/tool.mjs");
  });

  it("preserves canonical skill/tool bytes and regular-file counts", () => {
    const meta = {
      exec: ["z.txt"],
      symlinks: [{ path: "latest", target: "a.txt" }]
    } as const;
    const skill = bundleSkillFiles({
      "z.txt": new Uint8Array([0, 255, 7]),
      "SKILL.md": "---\nname: golden\ndescription: Golden\n---\n",
      "a.txt": "alpha"
    }, meta);
    const tool = bundleToolFiles({
      "z.txt": new Uint8Array([0, 255, 7]),
      "nested/tool.mjs": "export default () => 1;\n",
      "a.txt": "alpha"
    }, TOOL_MANIFEST, {
      exec: ["nested/tool.mjs"],
      symlinks: [{ path: "latest", target: "a.txt" }]
    });

    expect(createHash("sha256").update(skill.zip).digest("hex"))
      .toBe("1e1ced7941c77218325a9915fe6a3847d1c5a120c58d9ad794f1c3b3e7e2f2cd");
    expect(skill).toMatchObject({ fileCount: 3, compressedSize: 505 });
    expect(createHash("sha256").update(tool.zip).digest("hex"))
      .toBe("bfeebfea186cb1d40006f7b0f5021541e2d0885053a4eae2aea1ee0169a0ffe9");
    expect(tool).toMatchObject({ fileCount: 4, compressedSize: 722 });
  });

  it("preserves kind-specific map, content, and file-limit diagnostics", () => {
    expect(() => bundleSkillFiles(undefined as unknown as SkillFiles))
      .toThrow(/^Skill files map is required$/);
    expect(() => bundleToolFiles(undefined as unknown as SkillFiles, null as unknown as ToolBundleManifest))
      .toThrow(/^Tool files map is required$/);
    expect(() => bundleSkillFiles({}))
      .toThrow(/^Skill files map cannot be empty$/);
    expect(() => bundleToolFiles({}, TOOL_MANIFEST))
      .toThrow(/^Tool files map cannot be empty$/);

    const tooMany = Object.fromEntries(
      Array.from({ length: SKILL_BUNDLE_LIMITS.maxFiles + 1 }, (_, index) => [`f${index}`, ""])
    );
    expect(() => bundleSkillFiles(tooMany))
      .toThrow(/^Skill bundle exceeds 1000 file limit \(got 1001\)$/);
    expect(() => bundleToolFiles(tooMany, null as unknown as ToolBundleManifest))
      .toThrow(/^Tool bundle exceeds 1000 file limit \(got 1001\)$/);

    expect(() => bundleSkillFiles({ "../bad": 42 } as unknown as SkillFiles))
      .toThrow(/^Skill file "\.\.\/bad" must be a string or Uint8Array$/);
    expect(() => bundleToolFiles({ "../bad": 42 } as unknown as SkillFiles, TOOL_MANIFEST))
      .toThrow(/^Tool file "\.\.\/bad" must be a string or Uint8Array$/);
  });

  it("preserves validation precedence and cumulative expanded-size limits", () => {
    const invalidEntryManifest = { ...TOOL_MANIFEST, entry: "../bad.mjs" };
    expect(() => bundleToolFiles({ "nested/tool.mjs": 42 } as unknown as SkillFiles, invalidEntryManifest))
      .toThrow(/^bundle entry path contains '\.\.' segment: \.\.\/bad\.mjs$/);
    expect(() => bundleSkillFiles({ "plain.txt": "ok", "../bad": "bad" }))
      .toThrow(/^bundle entry path contains '\.\.' segment: \.\.\/bad$/);
    expect(() => bundleToolFiles({ "tool.json": "caller-owned" }, TOOL_MANIFEST))
      .toThrow(/^Tool bundle entry "nested\/tool\.mjs" must exist in files$/);
    expect(() => bundleToolFiles({ "tool.json": "caller-owned" }, { ...TOOL_MANIFEST, entry: "tool.json" }))
      .toThrow(/^Tool bundle files must not include reserved "tool\.json";/);

    const max = SKILL_BUNDLE_LIMITS.maxDecompressedBytes;
    expect(() => bundleSkillFiles({ "SKILL.md": apparentBytes(max), "extra.txt": apparentBytes(1) }))
      .toThrow(`Skill bundle exceeds decompressed cap of ${max} bytes`);
    expect(() => bundleToolFiles({
      "nested/tool.mjs": apparentBytes(max),
      "extra.txt": apparentBytes(1)
    }, TOOL_MANIFEST)).toThrow(`Tool bundle exceeds decompressed cap of ${max} bytes`);
  });
});
