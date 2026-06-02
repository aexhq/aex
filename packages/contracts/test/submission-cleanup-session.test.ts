import { describe, expect, it } from "vitest";
import { parseRunSubmissionRequest } from "../src/index.js";

const baseSubmission = {
  workspaceId: "workspace-1",
  idempotencyKey: "idem-1",
  submission: {
    model: "claude-haiku-4-5",
    prompt: ["say hello"],
    skills: [],
    agentsMd: [],
    files: [],
    mcpServers: []
  },
  secrets: {
    anthropic: { apiKey: "sk-ant-test" }
  }
} as const;

describe("submission cleanup.session — provider-neutral cleanup contract", () => {
  it("parses cleanup.session = retain", () => {
    const parsed = parseRunSubmissionRequest({ ...baseSubmission, cleanup: { session: "retain" } });
    expect(parsed.cleanup).toEqual({ session: "retain" });
  });

  it("parses cleanup.session = delete", () => {
    const parsed = parseRunSubmissionRequest({ ...baseSubmission, cleanup: { session: "delete" } });
    expect(parsed.cleanup).toEqual({ session: "delete" });
  });

  it("rejects unknown cleanup.session values", () => {
    expect(() => parseRunSubmissionRequest({ ...baseSubmission, cleanup: { session: "archive" } })).toThrow(/cleanup\.session/);
  });

  it("treats cleanup as undefined when omitted", () => {
    expect(parseRunSubmissionRequest(baseSubmission).cleanup).toBeUndefined();
  });
});
