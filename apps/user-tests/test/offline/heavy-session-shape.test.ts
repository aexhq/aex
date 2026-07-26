import { describe, expect, it } from "bun:test";
import { assertManagedShape, type CaseResult } from "../_fixtures/heavy-session-shape.js";

const SKILL_PREFIXES = [
  "heavy-alpha-managed-deepseek",
  "heavy-beta-managed-deepseek",
  "heavy-gamma-managed-deepseek"
] as const;

function baseResult(overrides: Partial<CaseResult> = {}): CaseResult {
  return {
    sessionId: "session-heavy-managed",
    runOutcome: "succeeded",
    runtime: "managed",
    // The provider the session RECORD reports — derived by the platform from the
    // model slug's creator prefix, not something the caller submitted.
    provider: "deepseek",
    probes: {
      system: "REF.system",
      instructions: "REF.instructions",
      prompt: "REF.prompt",
      out: ["REF-out-a", "REF-out-b", "REF-out-c"]
    },
    eventCount: 6,
    eventKinds: ["RUN_STARTED", "CUSTOM", "TOOL_CALL_START", "TOOL_CALL_RESULT", "TEXT_MESSAGE_CONTENT", "RUN_FINISHED"],
    eventTypeSet: ["RUN_STARTED", "CUSTOM", "TOOL_CALL_START", "TOOL_CALL_RESULT", "TEXT_MESSAGE_CONTENT", "RUN_FINISHED"],
    eventSource: "session.events.list",
    eventListError: null,
    fallbackEventCount: 6,
    listedEventCount: 6,
    toolCallStartCount: 1,
    toolCallResultCount: 1,
    notificationKinds: ["skill_loaded", "skill_loaded", "skill_loaded"],
    skillLoadedNames: [
      "heavy-alpha-managed-deepseek",
      "heavy-beta-managed-deepseek",
      "heavy-gamma-managed-deepseek"
    ],
    assistantTextJoined: "done",
    assistantTextEventCount: 1,
    terminalKind: "RUN_FINISHED",
    terminalData: {
      outcome: "succeeded",
      costUsd: 0.001,
      providerUsage: [],
      checkpoint: { checkpointId: "cp_1" }
    },
    fileCount: 1,
    fileSource: "session.files.list",
    fileListError: null,
    fallbackFileCount: 1,
    listedFileCount: 1,
    files: [{ filename: "files/heavy/result.txt", sizeBytes: 9, sample: "REF-out-a" }],
    outProbesFound: ["REF-out-a"],
    channelProbeSources: {
      system: ["assistantText"],
      instructions: ["toolCallStart"],
      prompt: ["toolCallResult"]
    },
    channelProbeMisses: [],
    leakedApiKey: false,
    streamErrors: [],
    ...overrides
  };
}

function captureError(fn: () => void): Error {
  try {
    fn();
  } catch (error) {
    return error instanceof Error ? error : new Error(String(error));
  }
  throw new Error("expected assertion to throw");
}

describe("heavy-session live assertion shape", () => {
  it("accepts a checkpoint-consistent RUN_FINISHED terminal", () => {
    expect(() => assertManagedShape(baseResult(), SKILL_PREFIXES)).not.toThrow();
  });

  it("requires RUN_STARTED before RUN_FINISHED", () => {
    const error = captureError(() =>
      assertManagedShape(
        baseResult({
          sessionId: "session-legacy-missing-start",
          terminalKind: "RUN_FINISHED",
          eventKinds: ["CUSTOM", "TOOL_CALL_START", "TOOL_CALL_RESULT", "TEXT_MESSAGE_CONTENT", "RUN_FINISHED"]
        }),
        SKILL_PREFIXES
      )
    );

    expect(error.message).toContain("RUN_STARTED must precede RUN_FINISHED");
    expect(error.message).toContain("sessionId=session-legacy-missing-start");
  });

  it("includes the session id in managed-session vocabulary failures", () => {
    const error = captureError(() =>
      assertManagedShape(
        baseResult({
          sessionId: "session-missing-tool-result",
          eventTypeSet: ["RUN_STARTED", "CUSTOM", "TOOL_CALL_START", "TEXT_MESSAGE_CONTENT", "RUN_FINISHED"]
        }),
        SKILL_PREFIXES
      )
    );

    expect(error.message).toContain('expected event type "TOOL_CALL_RESULT" was not observed');
    expect(error.message).toContain("sessionId=session-missing-tool-result");
  });

  it("includes selected-file download errors in the failure dump", () => {
    const error = captureError(() =>
      assertManagedShape(
        baseResult({
          sessionId: "session-file-download-error",
          files: [
            {
              filename: "files/heavy/report-1.txt",
              sizeBytes: 16,
              sample: "(download error: GET /api/sessions/session-file-download-error/files/file-1/download returned 503)"
            }
          ],
          outProbesFound: []
        }),
        SKILL_PREFIXES
      )
    );

    expect(error.message).toContain("file files/heavy/report-1.txt failed to download");
    expect(error.message).toContain("fileDownloadErrors:");
    expect(error.message).toContain("returned 503");
  });
});
