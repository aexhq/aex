import { describe, expect, it } from "vitest";
import { Models } from "../src/models.js";
import { normalizeSessionUnit, parseSessionUnitSubmission } from "../src/session-unit.js";

describe("normalizeSessionUnit (F25 — lean managed record → type-valid SessionUnit)", () => {
  it("fills empty aggregates for a lean record so array/page access never throws", () => {
    const lean = {
      id: "ses_abc",
      workspaceId: "ws_1",
      status: "idle",
      createdAt: "2026-07-02T00:00:00.000Z",
      updatedAt: "2026-07-02T00:01:00.000Z",
      terminalAt: "2026-07-02T00:01:00.000Z"
      // NO submission / attempts / events / outputs — the managed lean shape.
    };
    const unit = normalizeSessionUnit(lean);
    expect(unit.id).toBe("ses_abc");
    expect(unit.status).toBe("idle");
    // The type promises arrays + an event page — must be present at runtime.
    expect(Array.isArray(unit.attempts)).toBe(true);
    expect(unit.attempts).toEqual([]);
    expect(Array.isArray(unit.outputs)).toBe(true);
    expect(unit.outputs.map((o) => o.fileName)).toEqual([]);
    expect(unit.events.totalCount).toBe(0);
    expect(unit.events.entries).toEqual([]);
    expect(unit.events.truncated).toBe(false);
    expect(unit.rawEventPages).toEqual([]);
    expect(unit.outputCaptureFailures).toEqual([]);
    expect(unit.attemptCount).toBe(0);
    expect(unit.cleanupStatus).toBe("not_started");
    // submission is always present (fallback for a missing snapshot).
    expect(unit.submission.kind).toBe("submission");
  });

  it("passes through populated aggregates + costTelemetry verbatim", () => {
    const full = {
      id: "ses_x",
      workspaceId: "ws_1",
      status: "idle",
      cleanupStatus: "succeeded",
      createdAt: "2026-07-02T00:00:00.000Z",
      updatedAt: "2026-07-02T00:01:00.000Z",
      attemptCount: 2,
      attempts: [{ id: "a1", attemptNumber: 1, status: "ok", createdAt: "2026-07-02T00:00:00.000Z" }],
      events: { entries: [{ id: "e1", type: "TURN_STARTED", processedAt: "2026-07-02T00:00:00.000Z" }], totalCount: 5, truncated: true, nextCursor: "c1" },
      outputs: [{ id: "o1", fileName: "out.txt", byteSize: 3 }],
      costTelemetry: { schemaVersion: 1, billedCostUsd: 0.01 }
    };
    const unit = normalizeSessionUnit(full);
    expect(unit.attemptCount).toBe(2);
    expect(unit.attempts).toHaveLength(1);
    expect(unit.events.totalCount).toBe(5);
    expect(unit.events.truncated).toBe(true);
    expect(unit.events.nextCursor).toBe("c1");
    expect(unit.outputs[0]?.fileName).toBe("out.txt");
    expect(unit.cleanupStatus).toBe("succeeded");
    expect(unit.costTelemetry).toBeDefined();
  });

  it("tolerates a non-object payload without throwing", () => {
    const unit = normalizeSessionUnit(null);
    expect(unit.id).toBe("");
    expect(unit.attempts).toEqual([]);
    expect(unit.events.entries).toEqual([]);
  });

  it("uses the record's top-level model for the fallback submission (never claims a model the session did not use)", () => {
    // The hosted plane's GET /sessions/:id projects a flat record with `model`
    // at the top level and no `submission` snapshot. The fallback must echo
    // that model instead of fabricating the static default.
    const lean = {
      id: "ses_flat",
      workspaceId: "ws_1",
      status: "idle",
      createdAt: "2026-07-02T00:00:00.000Z",
      updatedAt: "2026-07-02T00:01:00.000Z",
      provider: "deepseek",
      model: "deepseek-v4-flash"
    };
    const unit = normalizeSessionUnit(lean);
    expect(unit.submission.kind).toBe("submission");
    expect(unit.submission.submission.model).toBe("deepseek-v4-flash");
  });
});

describe("parseSessionUnitSubmission", () => {
  it("parses a flat-shape snapshot verbatim", () => {
    const snapshot = {
      kind: "submission",
      submission: {
        model: "claude-haiku-4-5",
        system: "You are a helpful assistant.",
        prompt: ["build a thing"],
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

    const parsed = parseSessionUnitSubmission(snapshot);
    expect(parsed.kind).toBe("submission");
    if (parsed.kind !== "submission") return;
    expect(parsed.submission.model).toBe("claude-haiku-4-5");
    expect(parsed.submission.system).toBe("You are a helpful assistant.");
    expect(parsed.submission.prompt).toEqual(["build a thing"]);
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
      const parsed = parseSessionUnitSubmission(garbage);
      expect(parsed.kind).toBe("submission");
      if (parsed.kind !== "submission") return;
      expect(parsed.submission.model).toBe(Models.CLAUDE_HAIKU_4_5);
      expect(parsed.submission.prompt).toEqual([]);
      expect(parsed.submission.mcpServers).toEqual([]);
    }
  });

  it("drops malformed mcpServers entries without failing the whole parse", () => {
    const snapshot = {
      kind: "submission",
      submission: {
        model: "claude-haiku-4-5",
        prompt: ["hello"],
        mcpServers: [
          { name: "ok", url: "https://example.test/mcp" },
          { url: "missing-name" }
        ]
      }
    };
    const parsed = parseSessionUnitSubmission(snapshot);
    expect(parsed.kind).toBe("submission");
    if (parsed.kind !== "submission") return;
    expect(parsed.submission.mcpServers).toHaveLength(1);
    expect(parsed.submission.mcpServers[0]?.name).toBe("ok");
  });

  it("filters non-string prompt entries", () => {
    const snapshot = {
      kind: "submission",
      submission: {
        model: "claude-haiku-4-5",
        prompt: ["first", 42, null, "second"],
        mcpServers: []
      }
    };
    const parsed = parseSessionUnitSubmission(snapshot);
    if (parsed.kind !== "submission") throw new Error("unexpected kind");
    expect(parsed.submission.prompt).toEqual(["first", "second"]);
  });
});
