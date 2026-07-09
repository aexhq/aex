/**
 * WS9: `outputMode:'stream'` is capability-gated. A non-streamable provider
 * fails CLOSED at parse (hard reject, no silent downgrade) via the shared
 * STREAMABLE_SHAPES model.
 */
import { describe, expect, it } from "vitest";
import {
  STREAMABLE_SHAPES,
  isStreamableProvider,
  assertStreamableOutputMode,
  parseSessionSubmissionRequest
} from "../src/internal.js";

describe("streaming capability gate (WS9)", () => {
  it("STREAMABLE_SHAPES is the shape SSoT", () => {
    expect([...STREAMABLE_SHAPES]).toEqual(["anthropic", "openai_chat"]);
  });

  it("anthropic + openai-chat providers are streamable; gemini is not", () => {
    expect(isStreamableProvider("anthropic")).toBe(true);
    expect(isStreamableProvider("deepseek")).toBe(true);
    expect(isStreamableProvider("gemini")).toBe(false);
  });

  it("assertStreamableOutputMode fails closed for stream on a non-streamable provider", () => {
    expect(() => assertStreamableOutputMode("stream", "gemini")).toThrow(
      /'stream' is not supported for provider gemini/
    );
    expect(() => assertStreamableOutputMode("stream", "anthropic")).not.toThrow();
    expect(() => assertStreamableOutputMode("buffered", "gemini")).not.toThrow();
    expect(() => assertStreamableOutputMode(undefined, "gemini")).not.toThrow();
  });

  it("parseSessionSubmissionRequest hard-rejects stream on a non-streamable provider", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        workspaceId: "w1",
        idempotencyKey: "i1",
        provider: "gemini",
        submission: {
          model: "gemini-2.5-flash",
          prompt: ["hi"],
          agentsMd: [],
          files: [],
          mcpServers: [],
          outputMode: "stream"
        },
        secrets: { apiKeys: { gemini: "sk-x" } }
      })
    ).toThrow(/'stream' is not supported for provider gemini/);
  });

  it("parseSessionSubmissionRequest accepts stream on a streamable provider", () => {
    const parsed = parseSessionSubmissionRequest({
      workspaceId: "w1",
      idempotencyKey: "i1",
      provider: "deepseek",
      submission: {
        model: "deepseek-v4-flash",
        prompt: ["hi"],
        agentsMd: [],
        files: [],
        mcpServers: [],
        outputMode: "stream"
      },
      secrets: { apiKeys: { deepseek: "sk-x" } }
    });
    expect(parsed.submission.outputMode).toBe("stream");
  });
});
