import { describe, expect, it } from "vitest";
import { assertManagedShape, type CaseResult } from "../_fixtures/heavy-session-shape.js";

const SKILL_PREFIXES = [
  "heavy-alpha-managed-deepseek",
  "heavy-beta-managed-deepseek",
  "heavy-gamma-managed-deepseek"
] as const;

function baseResult(overrides: Partial<CaseResult> = {}): CaseResult {
  return {
    sessionId: "session-heavy-managed",
    attempts: 1,
    sessionStatus: "succeeded",
    runtime: "managed",
    provider: "deepseek",
    probes: {
      system: "REF.system",
      agentsMd: "REF.agents",
      prompt: "REF.prompt",
      out: ["REF-out-a", "REF-out-b", "REF-out-c"]
    },
    eventCount: 5,
    eventKinds: ["CUSTOM", "TOOL_CALL_START", "TOOL_CALL_RESULT", "TEXT_MESSAGE_CONTENT", "CUSTOM"],
    eventTypeSet: ["CUSTOM", "TOOL_CALL_START", "TOOL_CALL_RESULT", "TEXT_MESSAGE_CONTENT"],
    eventSource: "session.events().list",
    eventListError: null,
    fallbackEventCount: 5,
    listedEventCount: 5,
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
    terminalKind: "aex.session.succeeded",
    terminalData: { reason: "complete", runtimeExitCode: 0 },
    fileCount: 1,
    fileSource: "session.files().list",
    fileListError: null,
    fallbackFileCount: 1,
    listedFileCount: 1,
    files: [{ filename: "files/heavy/result.txt", sizeBytes: 9, sample: "REF-out-a" }],
    outProbesFound: ["REF-out-a"],
    channelProbeSources: {
      system: ["assistantText"],
      agentsMd: ["toolCallStart"],
      prompt: ["toolCallResult"]
    },
    channelProbeMisses: [],
    retryReasons: [],
    leakedDeepseekKey: false,
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
  it("accepts managed session terminals without legacy TURN_STARTED", () => {
    expect(() => assertManagedShape(baseResult(), SKILL_PREFIXES)).not.toThrow();
  });

  it("still requires TURN_STARTED before legacy TURN_FINISHED terminals", () => {
    const error = captureError(() =>
      assertManagedShape(
        baseResult({
          sessionId: "session-legacy-missing-start",
          terminalKind: "TURN_FINISHED",
          eventKinds: ["CUSTOM", "TOOL_CALL_START", "TOOL_CALL_RESULT", "TEXT_MESSAGE_CONTENT", "TURN_FINISHED"]
        }),
        SKILL_PREFIXES
      )
    );

    expect(error.message).toContain("legacy TURN_FINISHED stream did not include TURN_STARTED");
    expect(error.message).toContain("sessionId=session-legacy-missing-start");
  });

  it("includes the session id in managed-session vocabulary failures", () => {
    const error = captureError(() =>
      assertManagedShape(
        baseResult({
          sessionId: "session-missing-tool-result",
          eventTypeSet: ["CUSTOM", "TOOL_CALL_START", "TEXT_MESSAGE_CONTENT"]
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
