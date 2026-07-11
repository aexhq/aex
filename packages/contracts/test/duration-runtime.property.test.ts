import fc from "fast-check";
import { describe, expect, it } from "vitest";
import {
  MAX_SESSION_TIMEOUT_MS,
  MIN_SESSION_TIMEOUT_MS,
  RUNTIME_SIZES,
  parseDurationToMs,
  parseSessionTimeout,
  parseRuntimeSize
} from "../src/index.js";

/**
 * Property fuzz for the duration + runtime-size parsers. Invariants:
 *   - a well-formed duration NEVER yields NaN / negative / Infinity;
 *   - a malformed duration ALWAYS throws (never silently returns a bad number);
 *   - parseSessionTimeout enforces the [MIN, MAX] window exactly (in-range accepts,
 *     out-of-range throws) and is total (never NaN, never a non-Error throw);
 *   - parseRuntimeSize accepts exactly the known presets, rejects everything else.
 */

// The duration grammar is `\d+(\.\d+)?` + optional unit — NO exponent notation,
// so generate plain integer/fixed-decimal magnitudes (fc.double would emit "5e-324").
const num = fc.oneof(
  fc.integer({ min: 0, max: 10_000_000 }).map(String),
  fc.integer({ min: 0, max: 1_000_000 }).map((n) => (n / 100).toFixed(2))
);
const unit = fc.constantFrom("ms", "s", "m", "h", "");
const validDuration = fc.tuple(num, unit).map(([n, u]) => `${n}${u}`);

describe("parseDurationToMs (property)", { timeout: 0 }, () => {
  it("a well-formed duration is always a finite, non-negative number", () => {
    fc.assert(
      fc.property(validDuration, (d) => {
        const ms = parseDurationToMs(d);
        expect(Number.isFinite(ms)).toBe(true);
        expect(ms).toBeGreaterThanOrEqual(0);
        expect(Number.isNaN(ms)).toBe(false);
      }),
      { numRuns: 400 }
    );
  });

  it("unit factors are monotone: the same magnitude grows ms→s→m→h", () => {
    fc.assert(
      fc.property(fc.integer({ min: 1, max: 1000 }), (n) => {
        const ms = parseDurationToMs(`${n}ms`);
        const s = parseDurationToMs(`${n}s`);
        const m = parseDurationToMs(`${n}m`);
        const h = parseDurationToMs(`${n}h`);
        expect(ms).toBeLessThan(s);
        expect(s).toBeLessThan(m);
        expect(m).toBeLessThan(h);
      }),
      { numRuns: 100 }
    );
  });

  it("malformed durations always throw (never a silent bad number)", () => {
    const malformed = fc
      .string({ minLength: 1, maxLength: 16 })
      .filter((s) => !/^\s*\d+(?:\.\d+)?(ms|s|m|h)?\s*$/.test(s));
    fc.assert(
      fc.property(malformed, (s) => {
        expect(() => parseDurationToMs(s)).toThrow();
      }),
      { numRuns: 300 }
    );
  });
});

describe("parseSessionTimeout (property)", { timeout: 0 }, () => {
  it("undefined passes through; in-range durations are accepted exactly", () => {
    expect(parseSessionTimeout(undefined)).toBeUndefined();
    fc.assert(
      fc.property(fc.integer({ min: MIN_SESSION_TIMEOUT_MS, max: MAX_SESSION_TIMEOUT_MS }), (ms) => {
        const got = parseSessionTimeout(`${ms}ms`);
        expect(got).toBe(ms);
      }),
      { numRuns: 200 }
    );
  });

  it("out-of-window durations throw at both the floor and the ceiling", () => {
    fc.assert(
      fc.property(fc.integer({ min: 0, max: MIN_SESSION_TIMEOUT_MS - 1 }), (ms) => {
        expect(() => parseSessionTimeout(`${ms}ms`)).toThrow(/at least/);
      }),
      { numRuns: 100 }
    );
    fc.assert(
      fc.property(fc.integer({ min: MAX_SESSION_TIMEOUT_MS + 1, max: MAX_SESSION_TIMEOUT_MS * 4 }), (ms) => {
        expect(() => parseSessionTimeout(`${ms}ms`)).toThrow(/at most/);
      }),
      { numRuns: 100 }
    );
  });

  it("is total: any input either returns a number-or-undefined or throws an Error", () => {
    fc.assert(
      fc.property(fc.anything(), (input) => {
        try {
          const r = parseSessionTimeout(input);
          expect(r === undefined || (typeof r === "number" && Number.isFinite(r))).toBe(true);
        } catch (err) {
          expect(err).toBeInstanceOf(Error);
        }
      }),
      { numRuns: 300 }
    );
  });
});

describe("parseRuntimeSize (property)", { timeout: 0 }, () => {
  it("accepts exactly the known presets and rejects everything else", () => {
    for (const size of RUNTIME_SIZES) {
      expect(parseRuntimeSize(size)).toBe(size);
    }
    expect(parseRuntimeSize(undefined)).toBeUndefined();
    fc.assert(
      fc.property(
        fc.string({ maxLength: 24 }).filter((s) => !(RUNTIME_SIZES as readonly string[]).includes(s)),
        (s) => {
          expect(() => parseRuntimeSize(s)).toThrow();
        }
      ),
      { numRuns: 200 }
    );
  });
});
