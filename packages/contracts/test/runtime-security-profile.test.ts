import { describe, expect, it } from "bun:test";
import {
  evaluateRuntimeSecurityProfile,
  parseSessionSubmissionRequest,
  parseRuntimeSecurityProfile,
  resolveRuntimeSecurityProfile,
  serializeRuntimeSecurityProfile
} from "../src/internal.js";

const baseSubmission = {
  workspaceId: "workspace-1",
  idempotencyKey: "idem-1",
  submission: {
    model: "anthropic/claude-haiku-4-5",
    prompt: ["say hello"],    assets: { files: [], skills: [], tools: [], instructions: [] },
      builtinTools: "default",
    mcpServers: []
  },
  secrets: {}
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
    expect(resolveRuntimeSecurityProfile(undefined).defaultNetworkingMode).toBe("open");
  });

  it("keeps the profile as an additive submission snapshot field", () => {
    const parsed = parseSessionSubmissionRequest({
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
      parseSessionSubmissionRequest({
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
        packageCount: 1
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
    ]);

    expect(
      evaluateRuntimeSecurityProfile("developer", {
        networkingMode: "open",
        packageCount: 1,
        customerEnvVarCount: 1,
        mcpServerCount: 1
      })
    ).toEqual([]);
  });
});
