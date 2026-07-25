import { describe, expect, it } from "bun:test";
import { parseSessionSubmissionRequest } from "../src/internal.js";

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

describe("submission parser - runtimeKind", () => {
  it("surfaces an explicit runtimeKind on the parsed request", () => {
    for (const kind of ["container", "spot_container", "lambda"] as const) {
      const parsed = parseSessionSubmissionRequest(baseRequest({ runtimeKind: kind }));
      expect(parsed.runtimeKind).toBe(kind);
    }
  });

  it("omits runtimeKind when absent (downstream applies the spot_container default)", () => {
    expect(parseSessionSubmissionRequest(baseRequest()).runtimeKind).toBeUndefined();
  });

  it("is orthogonal to runtimeSize (both can be set together)", () => {
    const parsed = parseSessionSubmissionRequest(
      baseRequest({ runtimeKind: "spot_container", runtimeSize: "2cpu-8gb" })
    );
    expect(parsed.runtimeKind).toBe("spot_container");
    expect(parsed.runtimeSize).toBe("2cpu-8gb");
  });

  it("rejects an unknown runtimeKind through the full request parser", () => {
    expect(() => parseSessionSubmissionRequest(baseRequest({ runtimeKind: "fargate" }))).toThrow(
      /runtimeKind must be one of: container, spot_container, lambda/
    );
  });

  it("rejects a non-string runtimeKind through the full request parser", () => {
    expect(() => parseSessionSubmissionRequest(baseRequest({ runtimeKind: 3 }))).toThrow(
      /runtimeKind must be one of/
    );
  });

  it("still rejects the bare `runtime` field (reserved; use runtimeKind / runtimeSize)", () => {
    expect(() => parseSessionSubmissionRequest(baseRequest({ runtime: "container" }))).toThrow(
      /runtime is not an allowed field/
    );
  });
});
