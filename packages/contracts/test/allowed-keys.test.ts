import fc from "fast-check";
import { describe, expect, it, vi } from "vitest";
import { assertAllowedKeys, defineAllowedKeys } from "../src/allowed-keys.js";

describe("allowed-key assertion", () => {
  it("accepts empty and fully allowed records without mutation", () => {
    const allowed = defineAllowedKeys<{ readonly alpha?: unknown; readonly beta?: unknown }>()(
      "alpha",
      "beta"
    );
    const empty = {};
    const record = { beta: 2, alpha: 1 };
    const before = JSON.stringify(record);

    expect(() => assertAllowedKeys(empty, allowed, () => new Error("unused"))).not.toThrow();
    expect(() => assertAllowedKeys(record, allowed, () => new Error("unused"))).not.toThrow();
    expect(JSON.stringify(record)).toBe(before);
    expect(allowed).toEqual(["alpha", "beta"]);
  });

  it("reports only the first unknown key in native Object.keys order", () => {
    const ordinary = { allowed: true, later: true, earlier: true };
    const ordinaryError = vi.fn((key: string, keys: readonly string[]) => new Error(`${key}:${keys.join("|")}`));
    expect(() => assertAllowedKeys(ordinary, ["allowed"], ordinaryError)).toThrow("later:allowed");
    expect(ordinaryError).toHaveBeenCalledOnce();
    expect(ordinaryError).toHaveBeenCalledWith("later", ["allowed"]);

    const integerLike = { 10: true, 2: true, allowed: true, z: true };
    expect(() => assertAllowedKeys(integerLike, ["allowed"], (key) => new Error(key))).toThrow("2");
  });

  it("throws the caller-owned Error without wrapping away its structured identity", () => {
    class StructuredError extends Error {}
    const expected = new StructuredError("structured");
    let thrown: unknown;

    try {
      assertAllowedKeys({ unknown: true }, [], () => expected);
    } catch (error) {
      thrown = error;
    }

    expect(thrown).toBe(expected);
  });

  it("ignores inherited, non-enumerable, and symbol keys", () => {
    const inherited = { inherited: true };
    const record = Object.create(inherited) as Record<PropertyKey, unknown>;
    record.allowed = true;
    Object.defineProperty(record, "hidden", { enumerable: false, value: true });
    record[Symbol("symbol-key")] = true;

    expect(() => assertAllowedKeys(record, ["allowed"], (key) => new Error(key))).not.toThrow();
  });

  it("accepts exactly when every enumerable own string key is allowed", () => {
    fc.assert(
      fc.property(
        fc.uniqueArray(fc.tuple(fc.string(), fc.jsonValue()), {
          selector: ([key]) => key,
          maxLength: 20
        }),
        fc.uniqueArray(fc.string(), { maxLength: 20 }),
        (entries, allowed) => {
          const record = Object.fromEntries(entries);
          const firstUnknown = Object.keys(record).find((key) => !allowed.includes(key));
          let reported: string | undefined;
          try {
            assertAllowedKeys(record, allowed, (key) => {
              reported = key;
              return new Error(`unknown:${key}`);
            });
            expect(firstUnknown).toBeUndefined();
          } catch (error) {
            expect(error).toBeInstanceOf(Error);
            expect(reported).toBe(firstUnknown);
            expect((error as Error).message).toBe(`unknown:${firstUnknown}`);
          }
        }
      ),
      { numRuns: 500 }
    );
  });
});
