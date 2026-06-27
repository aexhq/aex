import fc from "fast-check";
import { describe, expect, it } from "vitest";
import { containsSecretLikeValue, redactString } from "../src/index.js";

/**
 * Property fuzz for the value-AGNOSTIC secret redactor. Two opposing risks:
 *   - UNDER-redaction: a secret-shaped run survives to a log/disk. The two real
 *     leaks this project hit (a pg password, a never-loaded Anthropic key) are
 *     the regression we fuzz against here.
 *   - OVER-redaction: a benign identifier (env name, module path, stack frame
 *     symbol) gets masked, gutting debuggability.
 * Real `redactString`/`containsSecretLikeValue` — no mocks, no value seeding.
 */

const REDACTED = "[REDACTED]";
const hexish = (n: number) => fc.string({ minLength: n, maxLength: n + 16 }).map((s) => s.replace(/[^A-Za-z0-9]/g, "a"));

const ALNUM = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789".split("");
function shannonBits(s: string): number {
  const counts = new Map<string, number>();
  for (const c of s) counts.set(c, (counts.get(c) ?? 0) + 1);
  let bits = 0;
  for (const n of counts.values()) {
    const p = n / s.length;
    bits -= p * Math.log2(p);
  }
  return bits;
}

// Generators that each produce a string matching ONE production secret SHAPE.
const secretShaped = fc.oneof(
  hexish(20).map((s) => `sk-ant-${s}`),
  hexish(24).map((s) => `sk-${s}`),
  hexish(20).map((s) => `apt_${s}`),
  hexish(20).map((s) => `ant_${s}`),
  // postgres connection string (whole URI must be redacted)
  fc.tuple(hexish(8), hexish(8)).map(([u, p]) => `postgresql://${u}:${p}@db.internal:5432/app`),
  // JWT-shaped header.payload.signature
  fc.tuple(hexish(10), hexish(10), hexish(10)).map(([a, b, c]) => `eyJ${a}.${b}.${c}`),
  // AWS access key id
  hexish(16).map((s) => `AKIA${s.toUpperCase().replace(/[^A-Z0-9]/g, "A").slice(0, 16)}`),
  // generic high-entropy blob: long, mixed-case, carries a digit, GENUINELY dense
  // (drawn uniformly from a 56-char alphabet so Shannon entropy actually clears
  // the redactor's gate — a low-entropy run like "xxxx…" is *correctly* ignored).
  fc
    .array(fc.constantFrom(...ALNUM), { minLength: 28, maxLength: 48 })
    .map((a) => a.join(""))
    .filter((s) => /[a-z]/.test(s) && /[A-Z]/.test(s) && /[0-9]/.test(s) && shannonBits(s) >= 3.5)
);

// Benign identifiers that MUST survive redaction unchanged.
const BENIGN_WORDS = ["internal", "modules", "esm", "loader", "create", "namespace", "async", "entry", "point", "with", "handler", "stream", "config", "probe", "url", "value", "node"];
const benignSnake = fc
  .array(fc.constantFrom(...BENIGN_WORDS), { minLength: 2, maxLength: 5 })
  .map((w) => w.join("_"));
const benignScreaming = fc
  .array(fc.constantFrom(...BENIGN_WORDS.map((w) => w.toUpperCase())), { minLength: 2, maxLength: 5 })
  .map((w) => w.join("_"));
const benignPath = fc
  .array(fc.constantFrom(...BENIGN_WORDS), { minLength: 2, maxLength: 6 })
  .map((w) => w.join("/"));
// digit-free mixed-case camelCase symbol, < 40 chars (the stack-frame escape hatch)
const benignCamel = fc
  .array(fc.constantFrom(...BENIGN_WORDS), { minLength: 2, maxLength: 4 })
  .map((w) => w.map((x, i) => (i === 0 ? x : x[0]!.toUpperCase() + x.slice(1))).join(""))
  .filter((s) => s.length < 40);

describe("secret redaction (property)", () => {
  it("masks every secret-shaped run (no under-redaction)", () => {
    fc.assert(
      fc.property(secretShaped, (secret) => {
        expect(containsSecretLikeValue(secret)).toBe(true);
        const out = redactString(secret);
        expect(out).toContain(REDACTED);
        expect(out).not.toBe(secret);
      }),
      { numRuns: 300 }
    );
  });

  it("masks a secret even when embedded in a benign log line", () => {
    fc.assert(
      fc.property(secretShaped, fc.constantFrom("loaded key ", "DATABASE_URL=", "auth -> "), (secret, prefix) => {
        const line = `${prefix}${secret} (done)`;
        const out = redactString(line);
        expect(out).toContain(REDACTED);
        // the secret-shaped run must not survive verbatim in the output
        expect(out).not.toContain(secret);
      }),
      { numRuns: 200 }
    );
  });

  it("never over-masks benign identifiers (env names, paths, stack symbols)", () => {
    fc.assert(
      fc.property(fc.oneof(benignSnake, benignScreaming, benignPath, benignCamel), (ident) => {
        expect(containsSecretLikeValue(ident)).toBe(false);
        expect(redactString(ident)).toBe(ident);
      }),
      { numRuns: 400 }
    );
  });

  it("is idempotent and total: redactString never throws and re-redaction is stable", () => {
    fc.assert(
      fc.property(fc.string({ maxLength: 200 }), (s) => {
        const once = redactString(s);
        const twice = redactString(once);
        expect(typeof once).toBe("string");
        // re-running over already-redacted text leaves the [REDACTED] markers intact
        expect(twice).toBe(once);
      }),
      { numRuns: 300 }
    );
  });
});
