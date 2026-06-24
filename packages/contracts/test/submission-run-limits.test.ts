import { describe, expect, it } from "vitest";
import { parseRunSubmissionRequest } from "../src/index.js";
// Reachability check: parseRunLimits + RunLimits are NOT on the public
// index export list by name; they ride the `export * from "./submission.js"`
// re-export and are pulled by the platform-only `@aexhq/contracts/internal`
// subpath. Import them from there to pin that surface.
import { parseRunLimits, type RunLimits } from "../src/internal.js";

function baseRequest() {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider: "anthropic" as const,
    submission: {
      model: "claude-haiku-4-5",
      prompt: ["hello"],
      skills: [],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: { apiKeys: { anthropic: "sk-anthropic-test" } }
  };
}

describe("parseRunLimits (shape + positivity gate)", () => {
  it("returns undefined when absent", () => {
    expect(parseRunLimits(undefined)).toBeUndefined();
  });

  it("accepts both fields together", () => {
    const limits: RunLimits = { maxConcurrentChildRuns: 200, maxSubagentDepth: 3 };
    expect(parseRunLimits(limits)).toEqual(limits);
  });

  it("accepts maxConcurrentChildRuns alone", () => {
    expect(parseRunLimits({ maxConcurrentChildRuns: 42 })).toEqual({ maxConcurrentChildRuns: 42 });
  });

  it("accepts maxSubagentDepth alone", () => {
    expect(parseRunLimits({ maxSubagentDepth: 5 })).toEqual({ maxSubagentDepth: 5 });
  });

  // An all-absent override carries no signal, so the parser collapses it to
  // `undefined` (same as an absent input, and like sibling parsers such as
  // parseRunWebhook / parseEnvironment) rather than landing an empty object on
  // the request. The resolver supplies platform defaults for absent fields.
  it("collapses an empty object input to undefined", () => {
    expect(parseRunLimits({})).toBeUndefined();
  });

  describe("maxConcurrentChildRuns boundaries", () => {
    it("accepts 1", () => {
      expect(parseRunLimits({ maxConcurrentChildRuns: 1 })).toEqual({ maxConcurrentChildRuns: 1 });
    });

    it("rejects 0", () => {
      expect(() => parseRunLimits({ maxConcurrentChildRuns: 0 })).toThrow(
        /limits\.maxConcurrentChildRuns must be a positive safe integer/
      );
    });

    it("rejects -1", () => {
      expect(() => parseRunLimits({ maxConcurrentChildRuns: -1 })).toThrow(
        /limits\.maxConcurrentChildRuns must be a positive safe integer/
      );
    });

    it("rejects 1.5 (fractional)", () => {
      expect(() => parseRunLimits({ maxConcurrentChildRuns: 1.5 })).toThrow(
        /limits\.maxConcurrentChildRuns must be a positive safe integer/
      );
    });

    it("rejects NaN", () => {
      expect(() => parseRunLimits({ maxConcurrentChildRuns: Number.NaN })).toThrow(
        /limits\.maxConcurrentChildRuns must be a positive safe integer/
      );
    });

    it("rejects Infinity", () => {
      expect(() => parseRunLimits({ maxConcurrentChildRuns: Number.POSITIVE_INFINITY })).toThrow(
        /limits\.maxConcurrentChildRuns must be a positive safe integer/
      );
    });

    it('rejects a numeric string "5"', () => {
      expect(() => parseRunLimits({ maxConcurrentChildRuns: "5" })).toThrow(
        /limits\.maxConcurrentChildRuns must be a positive safe integer/
      );
    });

    it("rejects true", () => {
      expect(() => parseRunLimits({ maxConcurrentChildRuns: true })).toThrow(
        /limits\.maxConcurrentChildRuns must be a positive safe integer/
      );
    });

    it("rejects null", () => {
      expect(() => parseRunLimits({ maxConcurrentChildRuns: null })).toThrow(
        /limits\.maxConcurrentChildRuns must be a positive safe integer/
      );
    });
  });

  describe("maxSubagentDepth boundaries", () => {
    it("accepts 1", () => {
      expect(parseRunLimits({ maxSubagentDepth: 1 })).toEqual({ maxSubagentDepth: 1 });
    });

    it("rejects 0", () => {
      expect(() => parseRunLimits({ maxSubagentDepth: 0 })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects -1", () => {
      expect(() => parseRunLimits({ maxSubagentDepth: -1 })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects 1.5 (fractional)", () => {
      expect(() => parseRunLimits({ maxSubagentDepth: 1.5 })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects NaN", () => {
      expect(() => parseRunLimits({ maxSubagentDepth: Number.NaN })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects Infinity", () => {
      expect(() => parseRunLimits({ maxSubagentDepth: Number.POSITIVE_INFINITY })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it('rejects a numeric string "5"', () => {
      expect(() => parseRunLimits({ maxSubagentDepth: "5" })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects true", () => {
      expect(() => parseRunLimits({ maxSubagentDepth: true })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects null", () => {
      expect(() => parseRunLimits({ maxSubagentDepth: null })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });
  });

  describe("non-object input", () => {
    it("rejects a string", () => {
      expect(() => parseRunLimits("nope")).toThrow(/limits must be an object/);
    });

    it("rejects a number", () => {
      expect(() => parseRunLimits(7)).toThrow(/limits must be an object/);
    });

    it("rejects an array", () => {
      expect(() => parseRunLimits([])).toThrow(/limits must be an object/);
    });

    it("rejects null", () => {
      expect(() => parseRunLimits(null)).toThrow(/limits must be an object/);
    });
  });

  // ACTUAL behavior: unknown/extra keys are REJECTED (the parser carries a
  // strict per-field allow-list, mirroring parseRunWebhook), NOT silently
  // ignored.
  it("rejects an unknown/extra key", () => {
    expect(() => parseRunLimits({ maxConcurrentChildRuns: 2, maxDepth: 9 })).toThrow(
      /limits\.maxDepth is not an allowed field/
    );
  });

  // No clamp at the parser: this is a SHAPE/positivity gate only. A value far
  // above any real ceiling passes through unchanged — the workspace/platform
  // ceilings are enforced by the resolver (resolveRunLimits in @aexhq/shared),
  // not here.
  it("does NOT clamp a huge-but-valid value (parse is shape-only)", () => {
    expect(parseRunLimits({ maxConcurrentChildRuns: 1e9, maxSubagentDepth: 1_000_000 })).toEqual({
      maxConcurrentChildRuns: 1e9,
      maxSubagentDepth: 1_000_000
    });
  });
});

describe("submission parser - limits", () => {
  it("surfaces limits on the parsed request when both fields are present", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      limits: { maxConcurrentChildRuns: 200, maxSubagentDepth: 3 }
    });
    expect(parsed.limits).toEqual({ maxConcurrentChildRuns: 200, maxSubagentDepth: 3 });
  });

  it("surfaces a single-field limits override verbatim on the parsed request", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      limits: { maxSubagentDepth: 4 }
    });
    expect(parsed.limits).toEqual({ maxSubagentDepth: 4 });
  });

  it("omits limits from the parsed request when absent", () => {
    expect(parseRunSubmissionRequest(baseRequest()).limits).toBeUndefined();
  });

  it("does NOT clamp an over-ceiling value at parse time (resolver clamps later)", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      limits: { maxConcurrentChildRuns: 1e9 }
    });
    expect(parsed.limits).toEqual({ maxConcurrentChildRuns: 1e9 });
  });

  it("rejects an invalid limits value through the full request parser", () => {
    expect(() =>
      parseRunSubmissionRequest({ ...baseRequest(), limits: { maxConcurrentChildRuns: 0 } })
    ).toThrow(/limits\.maxConcurrentChildRuns must be a positive safe integer/);
  });

  it("rejects an unknown subfield on the limits object through the full request parser", () => {
    expect(() =>
      parseRunSubmissionRequest({ ...baseRequest(), limits: { maxConcurrentChildRuns: 2, bogus: 1 } })
    ).toThrow(/limits\.bogus is not an allowed field/);
  });

  it("rejects a non-object limits value through the full request parser", () => {
    expect(() =>
      parseRunSubmissionRequest({ ...baseRequest(), limits: "nope" })
    ).toThrow(/limits must be an object/);
  });
});
