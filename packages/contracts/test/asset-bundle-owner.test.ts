import { describe, expect, it } from "vitest";
import { RESERVED_META_ENTRY } from "../src/index.js";
import {
  bundleSkillFiles,
  bundleToolFiles,
  hashSkillBundle,
  normalizeToolManifest
} from "../src/internal.js";

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
    expect(await hashSkillBundle(first.zip)).toMatch(/^sha256:[0-9a-f]{64}$/);
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
});
