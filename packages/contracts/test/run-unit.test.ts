import { describe, expect, it } from "vitest";
import { Models } from "../src/models.js";
import { parseRunUnitSubmission } from "../src/run-unit.js";

const HASH_HEX = "a".repeat(64);
const ASSET_SKILL = {
  kind: "asset",
  assetId: `asset_${HASH_HEX}`,
  name: "rules"
} as const;

describe("parseRunUnitSubmission", () => {
  it("parses a flat-shape snapshot verbatim", () => {
    const snapshot = {
      kind: "submission",
      submission: {
        model: "claude-haiku-4-5",
        system: "You are a helpful assistant.",
        prompt: ["build a thing"],
        skills: [ASSET_SKILL],
        mcpServers: [{ name: "context7", url: "https://example.test/mcp" }],
        environment: {
          networking: { mode: "limited", allowedHosts: ["example.test"] },
          packages: [{ name: "node", version: "22" }]
        },
        metadata: { team: "platform" },
        outputs: {
          allowedDirs: ["/workspace/out"],
          deniedDirs: ["node_modules"],
          captureTimeoutMs: 60000,
          maxFileBytes: 1234,
          maxTotalBytes: 5678,
          maxFiles: 9
        }
      }
    };

    const parsed = parseRunUnitSubmission(snapshot);
    expect(parsed.kind).toBe("submission");
    if (parsed.kind !== "submission") return;
    expect(parsed.submission.model).toBe("claude-haiku-4-5");
    expect(parsed.submission.system).toBe("You are a helpful assistant.");
    expect(parsed.submission.prompt).toEqual(["build a thing"]);
    expect(parsed.submission.skills).toEqual([ASSET_SKILL]);
    expect(parsed.submission.mcpServers).toEqual([
      { name: "context7", url: "https://example.test/mcp" }
    ]);
    expect(parsed.submission.environment?.networking?.mode).toBe("limited");
    expect(parsed.submission.environment?.packages?.[0]?.name).toBe("node");
    expect(parsed.submission.metadata).toEqual({ team: "platform" });
    expect(parsed.submission.outputs?.allowedDirs).toEqual(["/workspace/out"]);
    expect(parsed.submission.outputs?.deniedDirs).toEqual(["node_modules"]);
    expect(parsed.submission.outputs?.captureTimeoutMs).toBe(60000);
    expect(parsed.submission.outputs?.maxFileBytes).toBe(1234);
    expect(parsed.submission.outputs?.maxTotalBytes).toBe(5678);
    expect(parsed.submission.outputs?.maxFiles).toBe(9);
    expect("cleanup" in parsed).toBe(false);
  });

  it("falls back to an empty flat submission for null/garbage snapshots", () => {
    for (const garbage of [null, undefined, "string", 0, [], { random: "shape" }]) {
      const parsed = parseRunUnitSubmission(garbage);
      expect(parsed.kind).toBe("submission");
      if (parsed.kind !== "submission") return;
      expect(parsed.submission.model).toBe(Models.CLAUDE_HAIKU_4_5);
      expect(parsed.submission.prompt).toEqual([]);
      expect(parsed.submission.skills).toEqual([]);
      expect(parsed.submission.mcpServers).toEqual([]);
    }
  });

  it("drops malformed skills/mcpServers entries without failing the whole parse", () => {
    const snapshot = {
      kind: "submission",
      submission: {
        model: "claude-haiku-4-5",
        prompt: ["hello"],
        skills: [ASSET_SKILL, { kind: "nonsense" }, "not-an-object"],
        mcpServers: [
          { name: "ok", url: "https://example.test/mcp" },
          { url: "missing-name" }
        ]
      }
    };
    const parsed = parseRunUnitSubmission(snapshot);
    expect(parsed.kind).toBe("submission");
    if (parsed.kind !== "submission") return;
    expect(parsed.submission.skills).toHaveLength(1);
    expect(parsed.submission.mcpServers).toHaveLength(1);
    expect(parsed.submission.mcpServers[0]?.name).toBe("ok");
  });

  it("filters non-string prompt entries", () => {
    const snapshot = {
      kind: "submission",
      submission: {
        model: "claude-haiku-4-5",
        prompt: ["first", 42, null, "second"],
        skills: [],
        mcpServers: []
      }
    };
    const parsed = parseRunUnitSubmission(snapshot);
    if (parsed.kind !== "submission") throw new Error("unexpected kind");
    expect(parsed.submission.prompt).toEqual(["first", "second"]);
  });
});
