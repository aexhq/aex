import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { Skill } from "../../sdk/src/skill.js";
import { Tool } from "../../sdk/src/tool.js";
import {
  buildCliSkill,
  buildCliTool
} from "../src/host/start-submit.js";

const SKILL_MD = [
  "---",
  "name: parity-skill",
  'description: "Canonical parity skill"',
  "---",
  "",
  "Use the canonical bundle owner."
].join("\n");

describe("CLI/SDK asset authoring conformance", () => {
  it("produces byte-identical skill archives and hashes", async () => {
    const cli = await buildCliSkill(SKILL_MD, "skill.md");
    const sdk = await Skill.fromContent(SKILL_MD);
    const sdkDraft = sdk._takeDraftBundle();

    expect(cli.name).toBe(sdkDraft.name);
    expect(cli.description).toBe(sdkDraft.description);
    expect(cli.contentHash).toBe(sdkDraft.contentHash);
    expect(cli.bytes).toEqual(sdkDraft.bytes);
  });

  it("produces byte-identical tool archives, manifests, and hashes", async () => {
    const args = {
      name: "parity_tool",
      description: "Canonical parity tool",
      entry: "nested/tool.mjs",
      content: "export default async function () { return { ok: true }; }\n"
    } as const;
    const cli = await buildCliTool(args);
    const sdk = await Tool.fromFiles({
      name: args.name,
      description: args.description,
      input_schema: { type: "object", properties: {}, additionalProperties: true },
      entry: args.entry,
      files: { [args.entry]: args.content }
    });
    const sdkDraft = sdk._takeDraftBundle();

    expect(cli.ref.contentHash).toBe(sdkDraft.contentHash);
    expect(cli.bytes).toEqual(sdkDraft.bytes);
    expect(cli.ref).toMatchObject({
      name: sdkDraft.ref.name,
      description: sdkDraft.ref.description,
      input_schema: sdkDraft.ref.input_schema,
      entry: sdkDraft.ref.entry
    });
  });

  it("keeps the canonical implementation out of both consumer modules", () => {
    const cliSource = readFileSync(new URL("../src/host/start-submit.ts", import.meta.url), "utf8");
    const sdkAdapter = readFileSync(new URL("../../sdk/src/bundle.ts", import.meta.url), "utf8");

    expect(cliSource).toContain('from "@aexhq/contracts/internal"');
    expect(cliSource).not.toMatch(/function (?:collectBundleFiles|zipCollected|parseSkillFrontmatter|hashBytes)\b/);
    expect(sdkAdapter).toContain('from "@aexhq/contracts/internal"');
    expect(sdkAdapter).not.toContain("zipSync");
  });

  it("gives CLI authoring diagnostics start-flag provenance without changing SDK factories", async () => {
    await expect(buildCliSkill("no frontmatter", "aex start --skill"))
      .rejects.toThrow(/^aex start --skill: a skill name is required$/);
    await expect(buildCliTool({
      name: "parity_tool",
      description: "Canonical parity tool",
      entry: "tool.ts",
      content: "export default async function () {}\n"
    }, "aex start --tool")).rejects.toThrow(/^aex start --tool: entry must be a JS module \(\.js\/\.mjs\/\.cjs\)$/);

    await expect(Skill.fromContent("no frontmatter")).rejects.toThrow(
      /^Skill\.fromContent: a skill name is required — pass \{ name \}/
    );
    await expect(Tool.fromFiles({
      name: "parity_tool",
      description: "Canonical parity tool",
      input_schema: { type: "object", properties: {} },
      entry: "tool.ts",
      files: { "tool.ts": "export default async function () {}\n" }
    })).rejects.toThrow(
      /^Tool\.fromFiles: entry must be a JS module \(\.js\/\.mjs\/\.cjs\) that default-exports a function or \{ execute \}; got "tool\.ts"$/
    );
  });
});
