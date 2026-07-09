import { describe, expect, it } from "vitest";
import { parseSessionSubmissionRequest } from "../src/index.js";

const baseSubmission = {
  workspaceId: "workspace-1",
  idempotencyKey: "idem-1",
  submission: {
    model: "claude-haiku-4-5",
    prompt: ["say hello"],
    agentsMd: [],
    files: [],
    mcpServers: []
  },
  secrets: { apiKeys: { anthropic: "sk-ant-test" } }
} as const;

describe("submission proxy endpoint fields", () => {
  it("rejects proxyEndpoints as a removed public submission field", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [
          {
            name: "stripe",
            baseUrl: "https://api.stripe.com",
            authShape: { type: "bearer" },
            allowMethods: ["GET"],
            allowPathPrefixes: ["/v1/"]
          }
        ]
      })
    ).toThrow(/submission\.proxyEndpoints is not an allowed field/);
  });

  it("rejects secrets.proxyEndpointAuth as a removed public secrets field", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...baseSubmission,
        secrets: {
          ...baseSubmission.secrets,
          proxyEndpointAuth: [{ name: "stripe", value: { type: "bearer", token: "sk-test-token" } }]
        }
      })
    ).toThrow(/secrets\.proxyEndpointAuth is not an allowed field; permitted: apiKeys, mcpServers, envSecrets/);
  });
});
