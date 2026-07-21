import { describe, expect, it } from "vitest";
import {
  isJsonRecord,
  isJsonValue,
  isRecord,
  isStringLiteral
} from "../src/value-guards.js";

describe("value guards", () => {
  it("preserves the broad unknown-record contract", () => {
    expect(isRecord({})).toBe(true);
    expect(isRecord(Object.create(null))).toBe(true);
    expect(isRecord(new Date(0))).toBe(true);
    expect(isRecord([])).toBe(false);
    expect(isRecord(null)).toBe(false);
    expect(isRecord("record")).toBe(false);
  });

  it("recognizes recursive JSON values with finite numbers only", () => {
    expect(isJsonValue(null)).toBe(true);
    expect(isJsonValue(["value", true, 0, { nested: [1.5] }])).toBe(true);
    expect(isJsonValue({ inherited: Object.create({ ignored: Symbol("not-enumerable-own") }) })).toBe(true);

    for (const value of [undefined, 1n, Symbol("x"), () => undefined, Number.NaN, Infinity, -Infinity]) {
      expect(isJsonValue(value), String(value)).toBe(false);
    }
    expect(isJsonValue({ nested: [1, undefined] })).toBe(false);
    expect(isJsonValue({ nested: { bad: Number.NaN } })).toBe(false);
  });

  it("requires both record shape and recursively JSON-compatible values", () => {
    expect(isJsonRecord({ first: 1, second: { nested: true } })).toBe(true);
    expect(isJsonRecord(Object.create(null))).toBe(true);
    expect(isJsonRecord([])).toBe(false);
    expect(isJsonRecord(null)).toBe(false);
    expect(isJsonRecord({ bad: undefined })).toBe(false);
  });

  it("narrows exact readonly string-literal memberships", () => {
    const allowed = ["limited", "open"] as const;
    expect(isStringLiteral("limited", allowed)).toBe(true);
    expect(isStringLiteral("open", allowed)).toBe(true);
    expect(isStringLiteral("LIMITED", allowed)).toBe(false);
    expect(isStringLiteral(0, allowed)).toBe(false);
    expect(allowed).toEqual(["limited", "open"]);
  });
});
