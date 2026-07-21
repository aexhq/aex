import { describe, expect, it } from "vitest";
import { unzipSync } from "fflate";
import {
  WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN,
  assertWorkspaceInstructionResourceName
} from "@aexhq/contracts";
import { Instructions } from "../../src/instructions.js";

const acceptedNames = [
  "a",
  "a".repeat(63),
  "a".repeat(64),
  "a".repeat(65),
  "a".repeat(127),
  "a".repeat(128),
  "UpperCase",
  "repo.rules",
  "repo_rules",
  "repo-rules"
] as const;

const rejectedNames = [
  "",
  "a".repeat(129),
  "-leading",
  ".leading",
  "_leading",
  "two words",
  "slash/name",
  String.raw`backslash\name`,
  "café",
  "bad__name"
] as const;

describe("Instructions persisted resource names", () => {
  it.each(acceptedNames)("preserves accepted name %j without canonicalization", async (name) => {
    expect(WORKSPACE_INSTRUCTION_RESOURCE_NAME_PATTERN.test(name)).toBe(true);
    expect(() => assertWorkspaceInstructionResourceName(name, "name")).not.toThrow();

    const draft = await Instructions.fromContent("Follow the repository guide.", { name });
    expect(draft.name).toBe(name);
    const bundle = draft._takeDraftBundle();
    expect(bundle.name).toBe(name);
    expect(Object.keys(unzipSync(bundle.bytes))).toEqual(["AGENTS.md"]);
    expect(bundle.contentHash).toMatch(/^sha256:[0-9a-f]{64}$/);
  });

  it.each(rejectedNames)("rejects invalid name %j with SDK provenance", async (name) => {
    await expect(Instructions.fromContent("Follow the repository guide.", { name }))
      .rejects.toThrow(/^Instructions\.fromContent: name /);
  });
});
