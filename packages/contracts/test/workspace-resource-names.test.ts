import { describe, expect, it } from "bun:test";
import {
  WORKSPACE_FILE_RESOURCE_NAME_PATTERN,
  WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN,
  assertWorkspaceFileResourceName,
  assertWorkspaceInstructionResourceName,
  parseSubmission
} from "../src/index.js";

const acceptedNames = [
  "a",
  "a".repeat(63),
  "a".repeat(64),
  "a".repeat(65),
  "a".repeat(127),
  "a".repeat(128),
  "UpperCase",
  "report.final",
  "report_final",
  "report-final",
  "a.-_Z9"
] as const;

const rejectedNames = [
  "",
  "a".repeat(129),
  "-leading",
  ".leading",
  "_leading",
  "two words",
  "trailing ",
  "slash/name",
  String.raw`backslash\name`,
  "café",
  "../escape",
  "bad__name"
] as const;

const hash = "a".repeat(64);
const pinnedBase = {
  resourceId: `wres_${"1".repeat(32)}`,
  version: 1,
  assetId: `asset_${hash}`,
  contentHash: `sha256:${hash}`
} as const;

/** An instruction is pinned to its TEXT, so it carries no asset pair at all. */
const pinnedInstructionBase = {
  resourceId: `wres_${"1".repeat(32)}`,
  version: 1,
  textHash: `sha256:${hash}`
} as const;

describe("persisted workspace resource names", () => {
  it.each([
    ["file", WORKSPACE_FILE_RESOURCE_NAME_PATTERN, assertWorkspaceFileResourceName],
    ["instruction", WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN, assertWorkspaceInstructionResourceName]
  ] as const)("defines the %s contract independently", (_kind, pattern, assertName) => {
    for (const name of acceptedNames) {
      expect(pattern.test(name), name).toBe(true);
      expect(() => assertName(name, "resource.name"), name).not.toThrow();
    }
    for (const name of rejectedNames) {
      expect(() => assertName(name, "resource.name"), name).toThrow(/^resource\.name /);
    }
  });

  it("keeps separate file and instruction pattern identities", () => {
    expect(WORKSPACE_FILE_RESOURCE_NAME_PATTERN).not.toBe(WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN);
    expect(WORKSPACE_FILE_RESOURCE_NAME_PATTERN.source)
      .toBe(WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN.source);
  });

  it.each([
    ["file", "files", { ...pinnedBase, mountPath: "/workspace" }],
    ["instruction", "instructions", pinnedInstructionBase]
  ] as const)("applies the %s name contract to pinned refs", (kind, field, extra) => {
    for (const name of acceptedNames) {
      const assets = { files: [], skills: [], tools: [], instructions: [] } as Record<string, unknown>;
      assets[field] = [{ ...extra, kind, name }];
      expect(() => parseSubmission({
        model: "anthropic/claude-haiku-4-5",
        prompt: ["work"],
        assets,
        builtinTools: "none",
        mcpServers: []
      }), name).not.toThrow();
    }

    for (const name of rejectedNames) {
      const assets = { files: [], skills: [], tools: [], instructions: [] } as Record<string, unknown>;
      assets[field] = [{ ...extra, kind, name }];
      expect(() => parseSubmission({
        model: "anthropic/claude-haiku-4-5",
        prompt: ["work"],
        assets,
        builtinTools: "none",
        mcpServers: []
      }), name).toThrow(new RegExp(`submission\\.assets\\.${field}\\[0\\]\\.name`));
    }
  });

  it("reports the reserved separator separately from the character envelope", () => {
    expect(WORKSPACE_FILE_RESOURCE_NAME_PATTERN.test("bad__name")).toBe(true);
    expect(WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN.test("bad__name")).toBe(true);
    expect(() => assertWorkspaceFileResourceName("bad__name", "file.name"))
      .toThrow('file.name must not contain "__"');
    expect(() => assertWorkspaceInstructionResourceName("bad__name", "instruction.name"))
      .toThrow('instruction.name must not contain "__"');
  });

  it("does not broaden the independent skill-name grammar", () => {
    expect(() => parseSubmission({
      model: "anthropic/claude-haiku-4-5",
      prompt: ["work"],
      assets: {
        files: [],
        skills: [{
          ...pinnedBase,
          kind: "skill",
          name: "UpperCase",
          description: "Remains governed by the skill contract."
        }],
        tools: [],
        instructions: []
      },
      builtinTools: "none",
      mcpServers: []
    })).toThrow(/submission\.assets\.skills\[0\]\.name/);
  });
});
