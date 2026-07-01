import { describe, expect, it } from "vitest";
import { parseRunSubmissionRequest } from "../src/index.js";
// `parseRunMachine` / `RunMachine` ride the `export * from "./submission.js"`
// re-export (like parseRunLimits); pin them via the internal subpath.
import { parseRunMachine, type RunMachine } from "../src/internal.js";

function baseRequest(overrides: Record<string, unknown> = {}) {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider: "anthropic" as const,
    submission: {
      model: "claude-haiku-4-5",
      prompt: ["hello"],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: { apiKeys: { anthropic: "sk-anthropic-test" } },
    ...overrides
  };
}

describe("parseRunMachine (shape gate)", () => {
  it("returns undefined when absent", () => {
    expect(parseRunMachine(undefined)).toBeUndefined();
  });

  it("collapses a no-signal object to undefined", () => {
    expect(parseRunMachine({})).toBeUndefined();
  });

  it("preserves an explicit spot:true", () => {
    const machine: RunMachine = { spot: true };
    expect(parseRunMachine(machine)).toEqual({ spot: true });
  });

  it("preserves an explicit spot:false", () => {
    expect(parseRunMachine({ spot: false })).toEqual({ spot: false });
  });

  it("rejects a non-boolean spot", () => {
    expect(() => parseRunMachine({ spot: "yes" })).toThrow(/machine\.spot must be a boolean/);
    expect(() => parseRunMachine({ spot: 1 })).toThrow(/machine\.spot must be a boolean/);
    expect(() => parseRunMachine({ spot: null })).toThrow(/machine\.spot must be a boolean/);
  });

  it("rejects an unknown subfield", () => {
    expect(() => parseRunMachine({ spot: true, tier: "big" })).toThrow(
      /machine\.tier is not an allowed field/
    );
  });

  it("rejects a non-object input", () => {
    expect(() => parseRunMachine("shared-2x-8gb")).toThrow(/machine must be an object/);
    expect(() => parseRunMachine(7)).toThrow(/machine must be an object/);
    expect(() => parseRunMachine([])).toThrow(/machine must be an object/);
  });
});

describe("submission parser - machine", () => {
  it("surfaces machine.spot on the parsed request", () => {
    const parsed = parseRunSubmissionRequest(baseRequest({ machine: { spot: true } }));
    expect(parsed.machine).toEqual({ spot: true });
  });

  it("omits machine when absent (default is standard capacity / spot:false)", () => {
    expect(parseRunSubmissionRequest(baseRequest()).machine).toBeUndefined();
  });

  it("omits machine when the object carries no signal", () => {
    expect(parseRunSubmissionRequest(baseRequest({ machine: {} })).machine).toBeUndefined();
  });

  it("rejects an invalid machine value through the full request parser", () => {
    expect(() => parseRunSubmissionRequest(baseRequest({ machine: { spot: "nope" } }))).toThrow(
      /machine\.spot must be a boolean/
    );
  });

  it("rejects an unknown subfield on machine through the full request parser", () => {
    expect(() => parseRunSubmissionRequest(baseRequest({ machine: { spot: true, bogus: 1 } }))).toThrow(
      /machine\.bogus is not an allowed field/
    );
  });
});
