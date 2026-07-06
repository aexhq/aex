import { describe, expect, it } from "vitest";
import { assertManagedShape, type CaseResult } from "../_fixtures/heavy-session-shape.js";

const SKILL_PREFIXES = [
  "heavy-alpha-managed-deepseek",
  "heavy-beta-managed-deepseek",
  "heavy-gamma-managed-deepseek"
] as const;

function baseResult(overrides: Partial<CaseResult> = {}): CaseResult {
  return {
    runId: "run-heavy-managed",
    attempts: 1,
    runStatus: "succeeded",
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
    outputCount: 1,
    outputSource: "session.outputs().list",
    outputListError: null,
    fallbackOutputCount: 1,
    listedOutputCount: 1,
    outputs: [{ filename: "outputs/heavy/result.txt", sizeBytes: 9, sample: "REF-out-a" }],
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
  it("accepts managed session terminals without legacy RUN_STARTED", () => {
    expect(() => assertManagedShape(baseResult(), SKILL_PREFIXES)).not.toThrow();
  });

  it("still requires RUN_STARTED before legacy RUN_FINISHED terminals", () => {
    const error = captureError(() =>
      assertManagedShape(
        baseResult({
          runId: "run-legacy-missing-start",
          terminalKind: "RUN_FINISHED",
          eventKinds: ["CUSTOM", "TOOL_CALL_START", "TOOL_CALL_RESULT", "TEXT_MESSAGE_CONTENT", "RUN_FINISHED"]
        }),
        SKILL_PREFIXES
      )
    );

    expect(error.message).toContain("legacy RUN_FINISHED stream did not include RUN_STARTED");
    expect(error.message).toContain("runId=run-legacy-missing-start");
  });

  it("includes the run id in managed-session vocabulary failures", () => {
    const error = captureError(() =>
      assertManagedShape(
        baseResult({
          runId: "run-missing-tool-result",
          eventTypeSet: ["CUSTOM", "TOOL_CALL_START", "TEXT_MESSAGE_CONTENT"]
        }),
        SKILL_PREFIXES
      )
    );

    expect(error.message).toContain('expected event type "TOOL_CALL_RESULT" was not observed');
    expect(error.message).toContain("runId=run-missing-tool-result");
  });
});
