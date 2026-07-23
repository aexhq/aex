import { describe, expect, it } from "bun:test";
import {
  AEX_EVENT_TYPES,
  assertPinnedWorkspaceResource,
  parseSubmission,
  runnerEventToAexEvent,
  toAGUI,
  type AexEvent
} from "../src/index.js";

const hash = `sha256:${"a".repeat(64)}`;
const file = {
  kind: "file" as const,
  resourceId: `wres_${"1".repeat(32)}`,
  version: 3,
  assetId: `asset_${"a".repeat(64)}`,
  contentHash: hash,
  name: "input",
  mountPath: "/workspace"
};

describe("run lifecycle contract", () => {
  it("uses AG-UI RUN lifecycle names and carries real thread/run identity", () => {
    expect(AEX_EVENT_TYPES).toContain("RUN_FINISHED");
    expect(AEX_EVENT_TYPES).not.toContain("TURN_FINISHED");

    const started = runnerEventToAexEvent(
      { seq: 1, tMs: 2, kind: "runtime_started", data: {} },
      { sessionId: "session_1", runId: "run_9", baseMs: 1_000 }
    );
    expect(toAGUI(started!)).toEqual({
      type: "RUN_STARTED",
      timestamp: 1_002,
      threadId: "session_1",
      runId: "run_9"
    });
  });

  it("does not expose runtime completion as the public terminal", () => {
    const event = runnerEventToAexEvent(
      { seq: 2, tMs: 3, kind: "runtime_terminal", data: { reason: "success" } },
      { sessionId: "session_1", runId: "run_9", baseMs: 1_000 }
    );
    expect(event).toBeNull();
  });

  it("projects a committed terminal with checkpoint result and no sessionId alias", () => {
    const event: AexEvent = {
      specversion: "1.0",
      id: "session_1:8",
      source: "workflow",
      type: "RUN_FINISHED",
      subject: "session_1",
      threadId: "session_1",
      runId: "run_9",
      time: new Date(2_000).toISOString(),
      sequence: 8,
      data: {
        outcome: "succeeded",
        checkpoint: { checkpointId: "cp_1" },
        costUsd: 0,
        providerUsage: [],
        result: { checkpointId: "cp_1" }
      }
    };
    expect(toAGUI(event)).toEqual({
      type: "RUN_FINISHED",
      timestamp: 2_000,
      threadId: "session_1",
      runId: "run_9",
      result: { checkpointId: "cp_1" }
    });
  });
});

describe("workspace resource submission contract", () => {
  it("accepts only grouped, immutable, version-pinned resources", () => {
    expect(parseSubmission({
      model: "claude-haiku-4-5",
      prompt: ["work"],
      assets: { files: [file], skills: [], tools: [], instructions: [] },
      builtinTools: "none",
      mcpServers: []
    }).assets.files).toEqual([file]);

    expect(() => parseSubmission({
      model: "claude-haiku-4-5",
      prompt: ["work"],
      files: [file],
      mcpServers: []
    })).toThrow(/submission\.files is not an allowed field/);
  });

  it("rejects an asset id that does not identify contentHash", () => {
    expect(() => assertPinnedWorkspaceResource({ ...file, assetId: `asset_${"b".repeat(64)}` }, "assets.files[0]"))
      .toThrow(/same bytes/);
  });

  it("rejects resource ids the service cannot mint", () => {
    expect(() => assertPinnedWorkspaceResource({ ...file, resourceId: "file_1" }, "assets.files[0]"))
      .toThrow(/wres_<32 lowercase hex>/);
  });
});
