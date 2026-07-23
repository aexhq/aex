import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";
import {
  BUILTIN_TOOL_NAMES,
  buildSubagentAssetsInputSchema,
  buildSubagentBuiltinToolsInputSchema,
  parseSubagentAssetsInput,
  parseSubagentBuiltinToolsInput
} from "../src/subagent-runtime.js";

const HEX_A = "a".repeat(64);
const HEX_B = "b".repeat(64);

function allAssets() {
  return {
    files: [{
      kind: "file",
      resourceId: `wres_${"1".repeat(32)}`,
      version: 1,
      assetId: `asset_${HEX_A}`,
      contentHash: `sha256:${HEX_A}`,
      name: "input.txt",
      mountPath: "/workspace/input"
    }],
    skills: [{
      kind: "skill",
      resourceId: `wres_${"2".repeat(32)}`,
      version: 2,
      assetId: `asset_${HEX_B}`,
      contentHash: `sha256:${HEX_B}`,
      name: "research",
      description: "  keep surrounding whitespace  "
    }],
    tools: [{
      kind: "tool",
      resourceId: `wres_${"3".repeat(32)}`,
      version: 3,
      assetId: `asset_${HEX_A}`,
      contentHash: `sha256:${HEX_A}`,
      name: "custom_tool",
      description: "Custom tool",
      input_schema: {
        type: "object",
        properties: { count: { type: "number", maximum: 3 } }
      },
      entry: "src/index.mjs"
    }],
    instructions: [{
      kind: "instruction",
      resourceId: `wres_${"4".repeat(32)}`,
      version: 4,
      assetId: `asset_${HEX_B}`,
      contentHash: `sha256:${HEX_B}`,
      name: "AGENTS.md"
    }]
  };
}

describe("subagent nested input contract", () => {
  it("preserves valid entry objects, bytes, collection order, and builtin duplicates", () => {
    const input = allAssets();
    const snapshot = structuredClone(input);
    const parsed = parseSubagentAssetsInput(input);

    // expect<unknown>: the input/snapshot literals widen kind to string; the
    // deep equality is the assertion.
    expect<unknown>(parsed).toEqual(snapshot);
    expect(input).toEqual(snapshot);
    expect(parsed).not.toBe(input);
    expect(parsed?.files).not.toBe(input.files);
    expect<unknown>(parsed?.files[0]).toBe(input.files[0]);
    expect(parsed?.skills[0]?.description).toBe("  keep surrounding whitespace  ");
    expect(parsed?.tools[0]?.entry).toBe("src/index.mjs");

    const builtins = ["git", "bash", "git"] as const;
    const parsedBuiltins = parseSubagentBuiltinToolsInput(builtins);
    expect(parsedBuiltins).toEqual(builtins);
    expect(parsedBuiltins).not.toBe(builtins);
  });

  it.each([
    [null, "subagent: `assets` must contain files, skills, tools, and instructions arrays"],
    [[], "subagent: `assets` must contain files, skills, tools, and instructions arrays"],
    [{ files: [], skills: [], tools: [] }, "subagent: assets.instructions must be an array"],
    [{ files: [], skills: [], tools: [], instructions: [], extra: [] }, "subagent: assets.extra is not allowed"],
    [{ files: {}, skills: [], tools: [], instructions: [] }, "subagent: assets.files must be an array"]
  ])("retains exact outer-assets errors for %#", (input, message) => {
    expect(() => parseSubagentAssetsInput(input)).toThrowError(message);
  });

  it.each([
    ["files", { ...allAssets().files[0], kind: "skill" }, "subagent: assets.files[0].kind must be 'file'"],
    ["files", { ...allAssets().files[0], resourceId: "file_old" }, "subagent: assets.files[0].resourceId must match wres_<32 lowercase hex>"],
    ["files", { ...allAssets().files[0], version: 0 }, "subagent: assets.files[0].version must be a positive integer"],
    ["files", { ...allAssets().files[0], contentHash: `sha256:${HEX_A.toUpperCase()}` }, "subagent: assets.files[0].contentHash must be a sha256 digest"],
    ["files", { ...allAssets().files[0], assetId: `asset_${HEX_B}` }, "subagent: assets.files[0].assetId must identify the same bytes as contentHash"],
    ["files", { ...allAssets().files[0], name: "bad__name" }, "subagent: assets.files[0].name must not contain \"__\""],
    ["files", { ...allAssets().files[0], mountPath: "relative" }, "subagent: assets.files[0].mountPath must be an absolute path starting with '/'"],
    ["skills", { ...allAssets().skills[0], name: "skills" }, "subagent: assets.skills[0].name must not be a reserved skills name (skill, skills)"],
    ["skills", { ...allAssets().skills[0], description: " \t " }, "subagent: assets.skills[0].description must be non-empty and <= 2048 chars"],
    ["tools", { ...allAssets().tools[0], name: "bad__tool" }, "subagent: assets.tools[0].name must be a non-reserved tool name matching ^[a-z0-9][a-z0-9_-]{0,127}$"],
    ["tools", { ...allAssets().tools[0], input_schema: { type: "array" } }, "subagent: assets.tools[0].input_schema must be a JSON Schema object with type 'object'"],
    ["tools", { ...allAssets().tools[0], input_schema: { type: "object", x: Number.POSITIVE_INFINITY } }, "subagent: assets.tools[0].input_schema must be a JSON Schema object with type 'object'"],
    ["tools", { ...allAssets().tools[0], entry: "../index.mjs" }, "subagent: assets.tools[0].entry contains '..' segment"],
    ["instructions", { ...allAssets().instructions[0], name: "bad__name" }, "subagent: assets.instructions[0].name must not contain \"__\""]
  ])("rejects malformed nested %s refs before admission", (collection, entry, message) => {
    const input = allAssets() as Record<string, unknown[]>;
    input[collection] = [entry];
    expect(() => parseSubagentAssetsInput(input)).toThrowError(message);
  });

  it("rejects non-record entries, extra fields, sparse entries, and duplicate resource versions", () => {
    const nonRecord = allAssets();
    (nonRecord.files as unknown[])[0] = null;
    expect(() => parseSubagentAssetsInput(nonRecord)).toThrowError("subagent: assets.files[0] must be an object");

    const extra = allAssets();
    (extra.instructions[0] as Record<string, unknown>).extra = true;
    expect(() => parseSubagentAssetsInput(extra)).toThrowError("subagent: assets.instructions[0].extra is not allowed");

    const sparse = allAssets();
    sparse.files = new Array(1) as typeof sparse.files;
    expect(() => parseSubagentAssetsInput(sparse)).toThrowError("subagent: assets.files[0] must be an object");

    const duplicate = allAssets();
    duplicate.files.push({ ...duplicate.files[0]! });
    expect(() => parseSubagentAssetsInput(duplicate)).toThrowError(
      `subagent: assets.files[1] duplicates resource version ${duplicate.files[0]!.resourceId}:1`
    );
  });

  it("rejects cross-collection duplicate resource versions before API lookup", () => {
    const input = allAssets();
    input.instructions[0] = {
      ...input.instructions[0]!,
      resourceId: input.files[0]!.resourceId,
      version: input.files[0]!.version
    };
    expect(() => parseSubagentAssetsInput(input)).toThrowError(
      `subagent: assets.instructions[0] duplicates resource version ${input.files[0]!.resourceId}:1`
    );
  });

  // One-element tuples: bun's it.each spreads array cases into callback
  // arguments, so bare array inputs must be wrapped to arrive as one value.
  it.each([
    [undefined],
    ["default"],
    ["none"],
    [[]],
    [["git"]],
    [[...BUILTIN_TOOL_NAMES]]
  ])("accepts builtin selection %#", (input) => {
    expect(() => parseSubagentBuiltinToolsInput(input)).not.toThrow();
  });

  it("accepts dense builtin arrays with extra enumerable non-index properties", () => {
    const input = ["git"] as string[] & { metadata?: string };
    input.metadata = "ignored by JSON serialization";
    expect(parseSubagentBuiltinToolsInput(input)).toEqual(["git"]);
  });

  // One-element tuples for the same it.each array-spread reason as above.
  it.each([[null], [{}], [true], [1], ["unknown"], [["git", "unknown"]], [["git", null]], [new Array(1)]])(
    "retains the exact builtin rejection for %#",
    (input) => {
      expect(() => parseSubagentBuiltinToolsInput(input)).toThrowError(
        "subagent: `builtinTools` must be 'default', 'none', or an array of builtin names"
      );
    }
  );

  it("builds strict model schemas from the same closed vocabulary", () => {
    const assets = buildSubagentAssetsInputSchema();
    expect(assets).toMatchObject({
      type: "object",
      required: ["files", "skills", "tools", "instructions"],
      additionalProperties: false
    });
    const properties = assets.properties as Record<string, any>;
    expect(Object.keys(properties)).toEqual(["files", "skills", "tools", "instructions"]);
    expect(properties.files.items).toMatchObject({
      required: ["kind", "resourceId", "version", "assetId", "contentHash", "name", "mountPath"],
      additionalProperties: false
    });
    expect(properties.files.items.properties.resourceId.pattern).toBe("^wres_[0-9a-f]{32}$");
    expect(properties.tools.items.properties.input_schema).toMatchObject({
      type: "object",
      required: ["type"]
    });
    expect(properties.tools.items.properties.input_schema.properties.type).toEqual({ const: "object" });

    const builtins = buildSubagentBuiltinToolsInputSchema();
    expect((builtins.oneOf as any[])[1].items.enum).toEqual([...BUILTIN_TOOL_NAMES]);
  });

  it("keeps the owner on the narrow runtime subpath and out of the public root", () => {
    const sourceRoot = resolve(import.meta.dirname, "../src");
    const narrow = readFileSync(resolve(sourceRoot, "subagent-runtime.ts"), "utf8");
    const root = readFileSync(resolve(sourceRoot, "index.ts"), "utf8");
    expect(narrow).toContain('from "./subagent-input.js"');
    expect(root).not.toContain("subagent-input");
    expect(root).not.toContain("parseSubagentAssetsInput");
    expect(root).not.toContain("buildSubagentAssetsInputSchema");
  });
});
