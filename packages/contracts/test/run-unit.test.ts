import { describe, expect, it } from "vitest";
import { parseRunUnitSubmission } from "../src/run-unit.js";

const WS_UUID = "11111111-1111-4111-8111-111111111111";
const HASH_HEX = "a".repeat(64);
const R2_SKILL = {
  kind: "r2",
  path: `assets/${WS_UUID}/${HASH_HEX}`,
  hash: `sha256:${HASH_HEX}`,
  sizeBytes: 100,
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
        skills: [R2_SKILL],
        mcpServers: [{ name: "context7", url: "https://example.test/mcp" }],
        environment: {
          networking: { mode: "limited", allowedHosts: ["example.test"] },
          packages: [{ name: "node", version: "22" }]
        },
        metadata: { team: "platform" },
        outputDirs: ["/workspace/out"]
      },
      cleanup: { session: "delete" }
    };

    const parsed = parseRunUnitSubmission(snapshot);
    expect(parsed.kind).toBe("submission");
    if (parsed.kind !== "submission") return;
    expect(parsed.submission.model).toBe("claude-haiku-4-5");
    expect(parsed.submission.system).toBe("You are a helpful assistant.");
    expect(parsed.submission.prompt).toEqual(["build a thing"]);
    expect(parsed.submission.skills).toEqual([R2_SKILL]);
    expect(parsed.submission.mcpServers).toEqual([
      { name: "context7", url: "https://example.test/mcp" }
    ]);
    expect(parsed.submission.environment?.networking?.mode).toBe("limited");
    expect(parsed.submission.environment?.packages?.[0]?.name).toBe("node");
    expect(parsed.submission.metadata).toEqual({ team: "platform" });
    expect(parsed.submission.outputDirs).toEqual(["/workspace/out"]);
    expect(parsed.cleanup).toEqual({ session: "delete" });
  });

  it("falls back to an empty flat submission for null/garbage snapshots", () => {
    for (const garbage of [null, undefined, "string", 0, [], { random: "shape" }]) {
      const parsed = parseRunUnitSubmission(garbage);
      expect(parsed.kind).toBe("submission");
      if (parsed.kind !== "submission") return;
      expect(parsed.submission.model).toBe("");
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
        skills: [R2_SKILL, { kind: "nonsense" }, "not-an-object"],
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
