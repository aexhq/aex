import { describe, expect, it } from "vitest";
import { parseSessionSubmissionRequest, parseSessionWebhook } from "../src/index.js";

function baseRequest() {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider: "anthropic" as const,
    submission: {
      model: "claude-haiku-4-5",
      prompt: ["hello"],      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: { apiKeys: { anthropic: "sk-anthropic-test" } }
  };
}

describe("submission parser - webhook", () => {
  it("accepts a valid https webhook and surfaces it on the parsed request", () => {
    const parsed = parseSessionSubmissionRequest({
      ...baseRequest(),
      webhook: { url: "https://hooks.example.com/aex" }
    });
    expect(parsed.webhook).toEqual({ url: "https://hooks.example.com/aex" });
  });

  it("omits webhook from the parsed request when absent", () => {
    expect(parseSessionSubmissionRequest(baseRequest()).webhook).toBeUndefined();
  });

  it("rejects a non-https (http) callback URL", () => {
    expect(() =>
      parseSessionSubmissionRequest({ ...baseRequest(), webhook: { url: "http://hooks.example.com/aex" } })
    ).toThrow(/webhook\.url must use https/);
  });

  it("rejects a URL carrying userinfo", () => {
    expect(() =>
      parseSessionSubmissionRequest({ ...baseRequest(), webhook: { url: "https://user:pass@hooks.example.com/aex" } })
    ).toThrow(/webhook\.url must not contain userinfo/);
  });

  it("rejects an unknown subfield on the webhook object", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...baseRequest(),
        webhook: { url: "https://hooks.example.com/aex", events: ["run.started"] }
      })
    ).toThrow(/webhook\.events is not an allowed field/);
  });

  it("rejects a non-string url", () => {
    expect(() => parseSessionWebhook({ url: 123 })).toThrow(/webhook\.url must be a non-empty string/);
  });

  it("rejects a malformed absolute URL", () => {
    expect(() => parseSessionWebhook({ url: "not-a-url" })).toThrow(/must be a valid absolute URL/);
  });

  it("returns undefined for an absent webhook", () => {
    expect(parseSessionWebhook(undefined)).toBeUndefined();
  });
});
