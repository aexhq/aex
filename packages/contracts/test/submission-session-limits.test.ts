import { describe, expect, it } from "vitest";
// Reachability check: parseSessionLimits + SessionLimits are NOT on the public
// index export list by name; they ride the `export * from "./submission.js"`
// re-export and are pulled by the platform-only `@aexhq/contracts/internal`
// subpath. Import them from there to pin that surface.
import { parseSessionLimits, parseSessionSubmissionRequest, type SessionLimits } from "../src/internal.js";

function baseRequest() {
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider: "anthropic" as const,
    submission: {
      model: "claude-haiku-4-5",
      prompt: ["hello"],      assets: { files: [], skills: [], tools: [], instructions: [] },
      builtinTools: "default",
      mcpServers: []
    },
    secrets: { apiKeys: { anthropic: "sk-anthropic-test" } }
  };
}

describe("parseSessionLimits (shape + positivity gate)", () => {
  it("returns undefined when absent", () => {
    expect(parseSessionLimits(undefined)).toBeUndefined();
  });

  it("accepts both fields together", () => {
    const limits: SessionLimits = { maxConcurrentChildSessions: 200, maxSubagentDepth: 3 };
    expect(parseSessionLimits(limits)).toEqual(limits);
  });

  it("accepts maxConcurrentChildSessions alone", () => {
    expect(parseSessionLimits({ maxConcurrentChildSessions: 42 })).toEqual({ maxConcurrentChildSessions: 42 });
  });

  it("accepts maxSubagentDepth alone", () => {
    expect(parseSessionLimits({ maxSubagentDepth: 5 })).toEqual({ maxSubagentDepth: 5 });
  });

  // An all-absent override carries no signal, so the parser collapses it to
  // `undefined` (same as an absent input, and like sibling parsers such as
  // parseSessionWebhook / parseEnvironment) rather than landing an empty object on
  // the request. The resolver supplies platform defaults for absent fields.
  it("collapses an empty object input to undefined", () => {
    expect(parseSessionLimits({})).toBeUndefined();
  });

  describe("maxConcurrentChildSessions boundaries", () => {
    it("accepts 1", () => {
      expect(parseSessionLimits({ maxConcurrentChildSessions: 1 })).toEqual({ maxConcurrentChildSessions: 1 });
    });

    it("rejects 0", () => {
      expect(() => parseSessionLimits({ maxConcurrentChildSessions: 0 })).toThrow(
        /limits\.maxConcurrentChildSessions must be a positive safe integer/
      );
    });

    it("rejects -1", () => {
      expect(() => parseSessionLimits({ maxConcurrentChildSessions: -1 })).toThrow(
        /limits\.maxConcurrentChildSessions must be a positive safe integer/
      );
    });

    it("rejects 1.5 (fractional)", () => {
      expect(() => parseSessionLimits({ maxConcurrentChildSessions: 1.5 })).toThrow(
        /limits\.maxConcurrentChildSessions must be a positive safe integer/
      );
    });

    it("rejects NaN", () => {
      expect(() => parseSessionLimits({ maxConcurrentChildSessions: Number.NaN })).toThrow(
        /limits\.maxConcurrentChildSessions must be a positive safe integer/
      );
    });

    it("rejects Infinity", () => {
      expect(() => parseSessionLimits({ maxConcurrentChildSessions: Number.POSITIVE_INFINITY })).toThrow(
        /limits\.maxConcurrentChildSessions must be a positive safe integer/
      );
    });

    it('rejects a numeric string "5"', () => {
      expect(() => parseSessionLimits({ maxConcurrentChildSessions: "5" })).toThrow(
        /limits\.maxConcurrentChildSessions must be a positive safe integer/
      );
    });

    it("rejects true", () => {
      expect(() => parseSessionLimits({ maxConcurrentChildSessions: true })).toThrow(
        /limits\.maxConcurrentChildSessions must be a positive safe integer/
      );
    });

    it("rejects null", () => {
      expect(() => parseSessionLimits({ maxConcurrentChildSessions: null })).toThrow(
        /limits\.maxConcurrentChildSessions must be a positive safe integer/
      );
    });
  });

  describe("maxSubagentDepth boundaries", () => {
    it("accepts 1", () => {
      expect(parseSessionLimits({ maxSubagentDepth: 1 })).toEqual({ maxSubagentDepth: 1 });
    });

    it("rejects 0", () => {
      expect(() => parseSessionLimits({ maxSubagentDepth: 0 })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects -1", () => {
      expect(() => parseSessionLimits({ maxSubagentDepth: -1 })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects 1.5 (fractional)", () => {
      expect(() => parseSessionLimits({ maxSubagentDepth: 1.5 })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects NaN", () => {
      expect(() => parseSessionLimits({ maxSubagentDepth: Number.NaN })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects Infinity", () => {
      expect(() => parseSessionLimits({ maxSubagentDepth: Number.POSITIVE_INFINITY })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it('rejects a numeric string "5"', () => {
      expect(() => parseSessionLimits({ maxSubagentDepth: "5" })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects true", () => {
      expect(() => parseSessionLimits({ maxSubagentDepth: true })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });

    it("rejects null", () => {
      expect(() => parseSessionLimits({ maxSubagentDepth: null })).toThrow(
        /limits\.maxSubagentDepth must be a positive safe integer/
      );
    });
  });

  describe("maxSpendUsd boundaries (positive NUMBER — fractional USD allowed)", () => {
    it("accepts a fractional USD amount", () => {
      expect(parseSessionLimits({ maxSpendUsd: 2.5 })).toEqual({ maxSpendUsd: 2.5 });
    });

    it("accepts an integer USD amount", () => {
      expect(parseSessionLimits({ maxSpendUsd: 100 })).toEqual({ maxSpendUsd: 100 });
    });

    it("accepts it alongside the other fields", () => {
      expect(parseSessionLimits({ maxConcurrentChildSessions: 2, maxSubagentDepth: 3, maxSpendUsd: 5 })).toEqual({
        maxConcurrentChildSessions: 2,
        maxSubagentDepth: 3,
        maxSpendUsd: 5
      });
    });

    it("rejects 0", () => {
      expect(() => parseSessionLimits({ maxSpendUsd: 0 })).toThrow(
        /limits\.maxSpendUsd must be a positive finite number/
      );
    });

    it("rejects -1", () => {
      expect(() => parseSessionLimits({ maxSpendUsd: -1 })).toThrow(
        /limits\.maxSpendUsd must be a positive finite number/
      );
    });

    it("rejects NaN / Infinity", () => {
      expect(() => parseSessionLimits({ maxSpendUsd: Number.NaN })).toThrow(
        /limits\.maxSpendUsd must be a positive finite number/
      );
      expect(() => parseSessionLimits({ maxSpendUsd: Number.POSITIVE_INFINITY })).toThrow(
        /limits\.maxSpendUsd must be a positive finite number/
      );
    });

    it('rejects a numeric string "5"', () => {
      expect(() => parseSessionLimits({ maxSpendUsd: "5" })).toThrow(
        /limits\.maxSpendUsd must be a positive finite number/
      );
    });
  });

  describe("maxTurns boundaries (positive safe integer — WS11)", () => {
    it("accepts a positive integer", () => {
      expect(parseSessionLimits({ maxTurns: 40 })).toEqual({ maxTurns: 40 });
    });

    it("accepts it alongside the other fields", () => {
      expect(parseSessionLimits({ maxConcurrentChildSessions: 2, maxSubagentDepth: 3, maxSpendUsd: 5, maxTurns: 20 })).toEqual({
        maxConcurrentChildSessions: 2,
        maxSubagentDepth: 3,
        maxSpendUsd: 5,
        maxTurns: 20
      });
    });

    it("rejects 0 / -1 / fractional / string", () => {
      expect(() => parseSessionLimits({ maxTurns: 0 })).toThrow(/limits\.maxTurns must be a positive safe integer/);
      expect(() => parseSessionLimits({ maxTurns: -1 })).toThrow(/limits\.maxTurns must be a positive safe integer/);
      expect(() => parseSessionLimits({ maxTurns: 1.5 })).toThrow(/limits\.maxTurns must be a positive safe integer/);
      expect(() => parseSessionLimits({ maxTurns: "20" })).toThrow(/limits\.maxTurns must be a positive safe integer/);
    });

    it("survives the full request parser", () => {
      const parsed = parseSessionSubmissionRequest({ ...baseRequest(), limits: { maxTurns: 30 } });
      expect(parsed.limits).toEqual({ maxTurns: 30 });
    });
  });

  describe("maxStepsPerTurn boundaries (positive safe integer — doc 13 G2)", () => {
    it("accepts a positive integer", () => {
      expect(parseSessionLimits({ maxStepsPerTurn: 500 })).toEqual({ maxStepsPerTurn: 500 });
    });

    it("accepts it alongside the other fields", () => {
      expect(
        parseSessionLimits({ maxConcurrentChildSessions: 2, maxSubagentDepth: 3, maxSpendUsd: 5, maxTurns: 20, maxStepsPerTurn: 500 })
      ).toEqual({
        maxConcurrentChildSessions: 2,
        maxSubagentDepth: 3,
        maxSpendUsd: 5,
        maxTurns: 20,
        maxStepsPerTurn: 500
      });
    });

    it("rejects 0 / -1 / fractional / string", () => {
      expect(() => parseSessionLimits({ maxStepsPerTurn: 0 })).toThrow(/limits\.maxStepsPerTurn must be a positive safe integer/);
      expect(() => parseSessionLimits({ maxStepsPerTurn: -1 })).toThrow(/limits\.maxStepsPerTurn must be a positive safe integer/);
      expect(() => parseSessionLimits({ maxStepsPerTurn: 1.5 })).toThrow(/limits\.maxStepsPerTurn must be a positive safe integer/);
      expect(() => parseSessionLimits({ maxStepsPerTurn: "500" })).toThrow(/limits\.maxStepsPerTurn must be a positive safe integer/);
    });

    it("survives the full request parser", () => {
      const parsed = parseSessionSubmissionRequest({ ...baseRequest(), limits: { maxStepsPerTurn: 750 } });
      expect(parsed.limits).toEqual({ maxStepsPerTurn: 750 });
    });
  });

  describe("non-object input", () => {
    it("rejects a string", () => {
      expect(() => parseSessionLimits("nope")).toThrow(/limits must be an object/);
    });

    it("rejects a number", () => {
      expect(() => parseSessionLimits(7)).toThrow(/limits must be an object/);
    });

    it("rejects an array", () => {
      expect(() => parseSessionLimits([])).toThrow(/limits must be an object/);
    });

    it("rejects null", () => {
      expect(() => parseSessionLimits(null)).toThrow(/limits must be an object/);
    });
  });

  // ACTUAL behavior: unknown/extra keys are REJECTED (the parser carries a
  // strict per-field allow-list, mirroring parseSessionWebhook), NOT silently
  // ignored.
  it("rejects an unknown/extra key", () => {
    expect(() => parseSessionLimits({ maxConcurrentChildSessions: 2, maxDepth: 9 })).toThrow(
      /limits\.maxDepth is not an allowed field/
    );
  });

  // No clamp at the parser: this is a SHAPE/positivity gate only. A value far
  // above any real ceiling passes through unchanged — the workspace/platform
  // ceilings are enforced by the resolver (resolveSessionLimits in @aexhq/shared),
  // not here.
  it("does NOT clamp a huge-but-valid value (parse is shape-only)", () => {
    expect(parseSessionLimits({ maxConcurrentChildSessions: 1e9, maxSubagentDepth: 1_000_000 })).toEqual({
      maxConcurrentChildSessions: 1e9,
      maxSubagentDepth: 1_000_000
    });
  });
});

describe("submission parser - limits", () => {
  it("surfaces limits on the parsed request when both fields are present", () => {
    const parsed = parseSessionSubmissionRequest({
      ...baseRequest(),
      limits: { maxConcurrentChildSessions: 200, maxSubagentDepth: 3 }
    });
    expect(parsed.limits).toEqual({ maxConcurrentChildSessions: 200, maxSubagentDepth: 3 });
  });

  it("surfaces a single-field limits override verbatim on the parsed request", () => {
    const parsed = parseSessionSubmissionRequest({
      ...baseRequest(),
      limits: { maxSubagentDepth: 4 }
    });
    expect(parsed.limits).toEqual({ maxSubagentDepth: 4 });
  });

  it("omits limits from the parsed request when absent", () => {
    expect(parseSessionSubmissionRequest(baseRequest()).limits).toBeUndefined();
  });

  it("does NOT clamp an over-ceiling value at parse time (resolver clamps later)", () => {
    const parsed = parseSessionSubmissionRequest({
      ...baseRequest(),
      limits: { maxConcurrentChildSessions: 1e9 }
    });
    expect(parsed.limits).toEqual({ maxConcurrentChildSessions: 1e9 });
  });

  it("rejects an invalid limits value through the full request parser", () => {
    expect(() =>
      parseSessionSubmissionRequest({ ...baseRequest(), limits: { maxConcurrentChildSessions: 0 } })
    ).toThrow(/limits\.maxConcurrentChildSessions must be a positive safe integer/);
  });

  it("rejects an unknown subfield on the limits object through the full request parser", () => {
    expect(() =>
      parseSessionSubmissionRequest({ ...baseRequest(), limits: { maxConcurrentChildSessions: 2, bogus: 1 } })
    ).toThrow(/limits\.bogus is not an allowed field/);
  });

  it("rejects a non-object limits value through the full request parser", () => {
    expect(() =>
      parseSessionSubmissionRequest({ ...baseRequest(), limits: "nope" })
    ).toThrow(/limits must be an object/);
  });
});
