import { describe, expect, expectTypeOf, it } from "bun:test";
import type {
  JsonPrimitive as SubmissionJsonPrimitive,
  JsonValue as SubmissionJsonValue
} from "../src/submission.js";
import {
  isJsonRecord,
  isJsonValue,
  isRecord,
  isStringLiteral,
  type JsonPrimitive,
  type JsonValue
} from "../src/value-guards.js";

function narrowRecord(value: unknown): Record<string, unknown> | undefined {
  if (!isRecord(value)) return undefined;
  expectTypeOf(value).toEqualTypeOf<Record<string, unknown>>();
  return value;
}

function narrowJson(value: unknown): JsonValue | undefined {
  if (!isJsonValue(value)) return undefined;
  expectTypeOf(value).toEqualTypeOf<JsonValue>();
  return value;
}

function narrowJsonRecord(value: unknown): Record<string, JsonValue> | undefined {
  if (!isJsonRecord(value)) return undefined;
  expectTypeOf(value).toEqualTypeOf<Record<string, JsonValue>>();
  return value;
}

function narrowLiteral(value: unknown): "limited" | "open" | undefined {
  if (!isStringLiteral(value, ["limited", "open"] as const)) return undefined;
  expectTypeOf(value).toEqualTypeOf<"limited" | "open">();
  return value;
}

const validPrimitive: JsonPrimitive = 1;
const validJson: JsonValue = { nested: [validPrimitive, null] };
// @ts-expect-error Undefined is not a JSON value.
const invalidJson: JsonValue = undefined;

describe("value guard types", () => {
  it("keeps the existing public JSON aliases structurally identical", () => {
    expectTypeOf<SubmissionJsonPrimitive>().toEqualTypeOf<JsonPrimitive>();
    expectTypeOf<SubmissionJsonValue>().toEqualTypeOf<JsonValue>();
    expect(narrowRecord({ key: "value" })).toEqual({ key: "value" });
    expect(narrowJson(validJson)).toEqual(validJson);
    expect(narrowJsonRecord(validJson)).toEqual(validJson);
    expect(narrowLiteral("limited")).toBe("limited");
    expect(invalidJson).toBeUndefined();
  });
});
