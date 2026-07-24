/**
 * WS10: `responseFormat` discriminated union + parseResponseFormat mirror
 * OUTPUT_MODES/parseOutputMode; the schema field survives the full parser.
 */
import { describe, expect, it } from "bun:test";
import { parseResponseFormat, parseSessionSubmissionRequest, RESPONSE_FORMAT_KINDS } from "../src/internal.js";

describe("parseResponseFormat (WS10)", () => {
  it("kinds are the closed SSoT set", () => {
    expect([...RESPONSE_FORMAT_KINDS]).toEqual(["text", "json_schema"]);
  });

  it("defaults to undefined when absent", () => {
    expect(parseResponseFormat(undefined)).toBeUndefined();
    expect(parseResponseFormat(null)).toBeUndefined();
  });

  it("round-trips a json_schema format", () => {
    const schema = { type: "object", properties: { x: { type: "number" } }, required: ["x"] };
    expect(parseResponseFormat({ kind: "json_schema", schema, strict: true, name: "Point" })).toEqual({
      kind: "json_schema",
      schema,
      strict: true,
      name: "Point"
    });
  });

  it("accepts a bare text format", () => {
    expect(parseResponseFormat({ kind: "text" })).toEqual({ kind: "text" });
  });

  it("rejects an unknown kind", () => {
    expect(() => parseResponseFormat({ kind: "yaml_schema" })).toThrow(/responseFormat\.kind must be one of/);
  });

  it("rejects a json_schema with a non-object schema", () => {
    expect(() => parseResponseFormat({ kind: "json_schema", schema: "nope" })).toThrow(/schema must be a JSON/);
  });

  it("rejects an unknown subfield", () => {
    expect(() => parseResponseFormat({ kind: "json_schema", schema: {}, bogus: 1 })).toThrow(
      /is not an allowed field/
    );
  });
});

describe("responseFormat through the full submission parser (WS10)", () => {
  it("surfaces responseFormat on the parsed submission", () => {
    const parsed = parseSessionSubmissionRequest({
      workspaceId: "w1",
      idempotencyKey: "i1",
      submission: {
        model: "deepseek/deepseek-v4-flash",
        prompt: ["hi"],
        assets: { files: [], skills: [], tools: [], instructions: [] },
      builtinTools: "default",
        mcpServers: [],
        responseFormat: { kind: "json_schema", schema: { type: "object" } }
      },
      secrets: {}
    });
    expect(parsed.submission.responseFormat).toEqual({ kind: "json_schema", schema: { type: "object" } });
  });
});
