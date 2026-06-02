import { describe, expect, it } from "vitest";
import {
  evaluateRuntimeSecurityProfile,
  parseRunSubmissionRequest,
  parseRuntimeSecurityProfile,
  resolveRuntimeSecurityProfile,
  serializeRuntimeSecurityProfile
} from "../src/index.js";

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

describe("runtime security profiles", () => {
  it("validates and serializes the closed profile vocabulary", () => {
    expect(parseRuntimeSecurityProfile("strict")).toBe("strict");
    expect(parseRuntimeSecurityProfile("standard")).toBe("standard");
    expect(parseRuntimeSecurityProfile("developer")).toBe("developer");
    expect(parseRuntimeSecurityProfile(undefined)).toBeUndefined();
    expect(serializeRuntimeSecurityProfile("developer")).toBe("developer");
    expect(() => parseRuntimeSecurityProfile("prod-debug")).toThrow(
      /securityProfile must be one of: strict, standard, developer/
    );
  });

  it("defaults omitted profile policy to standard", () => {
    expect(resolveRuntimeSecurityProfile(undefined).name).toBe("standard");
    expect(resolveRuntimeSecurityProfile(undefined).defaultNetworkingMode).toBe("limited");
  });

  it("keeps the profile as an additive submission snapshot field", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        securityProfile: "strict"
      }
    });
    expect(parsed.submission.securityProfile).toBe("strict");
    expect(JSON.parse(JSON.stringify(parsed.submission)).securityProfile).toBe("strict");
  });

  it("rejects unknown submission security profiles", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          securityProfile: "root-shell"
        }
      })
    ).toThrow(/securityProfile must be one of/);
  });

  it("states strict and developer behavior boundaries", () => {
    expect(
      evaluateRuntimeSecurityProfile("strict", {
        networkingMode: "open",
        packageCount: 1,
        cleanupSession: "retain"
      })
    ).toEqual([
      {
        field: "environment.networking.mode",
        reason: "strict requires limited networking"
      },
      {
        field: "environment.packages",
        reason: "strict does not allow runtime package installs"
      },
      {
        field: "cleanup.session",
        reason: "strict requires provider sessions to be cleaned up"
      }
    ]);

    expect(
      evaluateRuntimeSecurityProfile("developer", {
        networkingMode: "open",
        packageCount: 1,
        customerEnvVarCount: 1,
        proxyEndpointCount: 1,
        mcpServerCount: 1,
        cleanupSession: "retain"
      })
    ).toEqual([]);
  });
});
