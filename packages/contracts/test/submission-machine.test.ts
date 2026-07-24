import { describe, expect, it } from "bun:test";
// `parseSessionMachine` / `SessionMachine` ride the `export * from "./submission.js"`
// re-export (like parseSessionLimits); pin them via the internal subpath.
import { parseSessionMachine, parseSessionSubmissionRequest, type SessionMachine } from "../src/internal.js";

function baseRequest(overrides: Record<string, unknown> = {}) {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    submission: {
      model: "anthropic/claude-haiku-4-5",
      prompt: ["hello"],
      assets: { files: [], skills: [], tools: [], instructions: [] },
      builtinTools: "default",
      mcpServers: []
    },
    secrets: {},
    ...overrides
  };
}

describe("parseSessionMachine (shape gate)", () => {
  it("returns undefined when absent", () => {
    expect(parseSessionMachine(undefined)).toBeUndefined();
  });

  it("collapses a no-signal object to undefined", () => {
    expect(parseSessionMachine({})).toBeUndefined();
  });

  it("preserves an explicit spot:true", () => {
    const machine: SessionMachine = { spot: true };
    expect(parseSessionMachine(machine)).toEqual({ spot: true });
  });

  it("preserves an explicit spot:false", () => {
    expect(parseSessionMachine({ spot: false })).toEqual({ spot: false });
  });

  it("rejects a non-boolean spot", () => {
    expect(() => parseSessionMachine({ spot: "yes" })).toThrow(/machine\.spot must be a boolean/);
    expect(() => parseSessionMachine({ spot: 1 })).toThrow(/machine\.spot must be a boolean/);
    expect(() => parseSessionMachine({ spot: null })).toThrow(/machine\.spot must be a boolean/);
  });

  it("rejects an unknown subfield", () => {
    expect(() => parseSessionMachine({ spot: true, tier: "big" })).toThrow(
      /machine\.tier is not an allowed field/
    );
  });

  it("rejects a non-object input", () => {
    expect(() => parseSessionMachine("2cpu-8gb")).toThrow(/machine must be an object/);
    expect(() => parseSessionMachine(7)).toThrow(/machine must be an object/);
    expect(() => parseSessionMachine([])).toThrow(/machine must be an object/);
  });
});

describe("submission parser - machine", () => {
  it("surfaces machine.spot on the parsed request", () => {
    const parsed = parseSessionSubmissionRequest(baseRequest({ machine: { spot: true } }));
    expect(parsed.machine).toEqual({ spot: true });
  });

  it("omits machine when absent (default is standard capacity / spot:false)", () => {
    expect(parseSessionSubmissionRequest(baseRequest()).machine).toBeUndefined();
  });

  it("omits machine when the object carries no signal", () => {
    expect(parseSessionSubmissionRequest(baseRequest({ machine: {} })).machine).toBeUndefined();
  });

  it("rejects an invalid machine value through the full request parser", () => {
    expect(() => parseSessionSubmissionRequest(baseRequest({ machine: { spot: "nope" } }))).toThrow(
      /machine\.spot must be a boolean/
    );
  });

  it("rejects an unknown subfield on machine through the full request parser", () => {
    expect(() => parseSessionSubmissionRequest(baseRequest({ machine: { spot: true, bogus: 1 } }))).toThrow(
      /machine\.bogus is not an allowed field/
    );
  });
});
