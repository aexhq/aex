import fc from "fast-check";
import { describe, expect, it } from "vitest";
import { parseProxyAuthShape, validateProxyAuth } from "../src/internal.js";

/**
 * Property fuzz for the proxy cross-field validators:
 *   - validateProxyAuth: the endpoints policy list and the secret auth-value
 *     list must agree 1:1 on names AND on auth-shape types;
 *   - parseProxyAuthShape: the per-endpoint auth shape parser.
 * Real validators (the SDK/CLI run these client-side before the wire) — no mocks.
 */

const AUTH_TYPES = ["none", "bearer", "basic", "header", "query"] as const;
type AuthType = (typeof AUTH_TYPES)[number];

const distinctNames = fc
  .uniqueArray(
    fc.string({ minLength: 1, maxLength: 8 }).map((s) => s.replace(/[^a-z]/g, "a") || "a"),
    { minLength: 1, maxLength: 5, selector: (s) => s }
  );

// A consistent endpoint/auth pairing keyed by name → type.
const pairing = distinctNames.chain((names) =>
  fc.tuple(...names.map(() => fc.constantFrom<AuthType>(...AUTH_TYPES))).map((types) =>
    names.map((name, i) => ({ name, type: types[i]! as AuthType }))
  )
);

function endpointsOf(pairs: ReadonlyArray<{ name: string; type: AuthType }>) {
  return pairs.map((p) => ({ name: p.name, authShape: { type: p.type } })) as never;
}
function authOf(pairs: ReadonlyArray<{ name: string; type: AuthType }>) {
  return pairs.map((p) => ({ name: p.name, value: { type: p.type } })) as never;
}

describe("validateProxyAuth (property)", () => {
  it("accepts a faithful 1:1 name+type pairing", () => {
    fc.assert(
      fc.property(pairing, (pairs) => {
        expect(() => validateProxyAuth(endpointsOf(pairs), authOf(pairs))).not.toThrow();
      }),
      { numRuns: 200 }
    );
  });

  it("rejects a type mismatch on a matched name", () => {
    fc.assert(
      fc.property(
        pairing.filter((p) => p.length > 0),
        fc.nat(),
        (pairs, idx) => {
          const i = idx % pairs.length;
          const auth = pairs.map((p, j) =>
            j === i ? { name: p.name, value: { type: nextType(p.type) } } : { name: p.name, value: { type: p.type } }
          ) as never;
          expect(() => validateProxyAuth(endpointsOf(pairs), auth)).toThrow(/does not match/);
        }
      ),
      { numRuns: 200 }
    );
  });

  it("rejects an endpoint with no matching auth entry", () => {
    fc.assert(
      fc.property(
        pairing.filter((p) => p.length > 0),
        (pairs) => {
          // drop the last auth entry → its endpoint is now unmatched
          const auth = authOf(pairs.slice(0, -1));
          expect(() => validateProxyAuth(endpointsOf(pairs), auth)).toThrow(/missing a matching/);
        }
      ),
      { numRuns: 150 }
    );
  });

  it("rejects an auth entry whose name has no endpoint, and duplicate auth names", () => {
    fc.assert(
      fc.property(
        pairing.filter((p) => p.length > 0),
        (pairs) => {
          const orphan = [...authOf(pairs), { name: "zzz_orphan", value: { type: "bearer" } }] as never;
          expect(() => validateProxyAuth(endpointsOf(pairs), orphan)).toThrow(/no matching proxyEndpoints/);
          const dup = [...authOf(pairs), { name: pairs[0]!.name, value: { type: pairs[0]!.type } }] as never;
          expect(() => validateProxyAuth(endpointsOf(pairs), dup)).toThrow(/duplicate name/);
        }
      ),
      { numRuns: 150 }
    );
  });
});

describe("parseProxyAuthShape (property)", () => {
  it("round-trips the keyless shapes and named shapes", () => {
    for (const type of ["none", "bearer", "basic"] as const) {
      expect(parseProxyAuthShape({ type }, "f")).toEqual({ type });
    }
    fc.assert(
      fc.property(
        fc.constantFrom("header", "query"),
        fc.string({ minLength: 1, maxLength: 32 }).map((s) => s.replace(/[^a-zA-Z0-9_.-]/g, "x") || "x"),
        (type, name) => {
          expect(parseProxyAuthShape({ type, name }, "f")).toEqual({ type, name });
        }
      ),
      { numRuns: 150 }
    );
  });

  it("rejects unknown types, missing names, and extra keys (total: always Error)", () => {
    fc.assert(
      fc.property(
        fc.oneof(
          fc.record({ type: fc.string().filter((t) => !AUTH_TYPES.includes(t as AuthType)) }),
          fc.constant({ type: "header" }), // missing name
          fc.constant({ type: "query" }), // missing name
          fc.constant({ type: "bearer", extra: 1 }), // extra key
          fc.constant({}),
          fc.constant({ type: 123 })
        ),
        (input) => {
          expect(() => parseProxyAuthShape(input, "f")).toThrow();
        }
      ),
      { numRuns: 200 }
    );
  });
});

function nextType(t: AuthType): AuthType {
  const i = AUTH_TYPES.indexOf(t);
  return AUTH_TYPES[(i + 1) % AUTH_TYPES.length]!;
}
