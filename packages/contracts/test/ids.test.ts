/**
 * The identifier authority's own contract.
 *
 * `ids.ts` is the only module in the workspace allowed to state an id shape, so
 * this suite is the only place the shape is asserted from a literal. Every other
 * test mints its fixtures with `newId`.
 */
import { describe, expect, it } from "bun:test";
import {
  ID_KINDS,
  ID_PREFIXES,
  assertId,
  idKindOf,
  idPattern,
  idPatternSource,
  isId,
  newId,
  type IdKind
} from "../src/ids.js";

describe("ID_PREFIXES", () => {
  it("declares every entity kind exactly once, with a unique prefix", () => {
    expect(ID_PREFIXES).toEqual({
      workspace: "wsp",
      session: "ses",
      resource: "wres",
      mcp: "mcp",
      secret: "sec",
      org: "org",
      team: "team",
      user: "usr",
      apiKey: "key",
      idempotency: "idem"
    });
    const prefixes = Object.values(ID_PREFIXES);
    expect(new Set(prefixes).size).toBe(prefixes.length);
  });

  it("uses only lowercase letters in a prefix, so `<prefix>_` never needs a case-insensitive match", () => {
    for (const prefix of Object.values(ID_PREFIXES)) {
      expect(prefix).toMatch(/^[a-z]+$/);
    }
  });

  it("ID_KINDS covers ID_PREFIXES", () => {
    expect([...ID_KINDS].sort()).toEqual((Object.keys(ID_PREFIXES) as IdKind[]).sort());
  });
});

describe("newId", () => {
  it("mints `<prefix>_<32 lowercase hex>` for every kind", () => {
    for (const kind of ID_KINDS) {
      const value = newId(kind);
      expect(value).toMatch(new RegExp(`^${ID_PREFIXES[kind]}_[0-9a-f]{32}$`));
      expect(isId(kind, value)).toBe(true);
    }
  });

  it("is unique across mints (32 hex = 128 bits)", () => {
    const minted = new Set(Array.from({ length: 512 }, () => newId("workspace")));
    expect(minted.size).toBe(512);
  });

  it("refuses an unknown kind rather than minting a prefixless id", () => {
    expect(() => newId("nope" as IdKind)).toThrow('newId: unknown id kind "nope"');
  });
});

describe("isId", () => {
  it("is exact: one kind's id is never another kind's id", () => {
    for (const kind of ID_KINDS) {
      const value = newId(kind);
      for (const other of ID_KINDS) {
        expect(isId(other, value)).toBe(other === kind);
      }
    }
  });

  it("rejects the four dead workspace-id forms this module replaced", () => {
    // The pre-2026-07-25 forms: dashed uuid (Aurora PK, log lines, S3 keys),
    // dash-free hex (API-key field), the generated `public_id`'s uppercase
    // variant, and the coercer's output for a non-uuid input.
    for (const dead of [
      "5fc4b90e-55af-46cf-9938-b70f988e431d",
      "5fc4b90e55af46cf9938b70f988e431d",
      "wsp_5FC4B90E55AF46CF9938B70F988E431D",
      "wsabc123",
      "ws-abc-123",
      "wsp_example",
      "wsp_5fc4b90e55af46cf9938b70f988e431",
      "wsp_5fc4b90e55af46cf9938b70f988e431dd",
      "wsp_5fc4b90e-55af-46cf-9938-b70f988e431d"
    ]) {
      expect(isId("workspace", dead)).toBe(false);
    }
  });

  it("rejects non-strings without throwing", () => {
    for (const value of [undefined, null, 42, {}, [], Symbol("x")]) {
      expect(isId("workspace", value)).toBe(false);
    }
  });

  it("is case-sensitive — an uppercase-hex id is not an id anywhere", () => {
    // The bug this closes: seven copies matched `/i` and telemetry's did not, so
    // an uppercase id passed every boundary and threw at the span attribute.
    const upper = newId("workspace").toUpperCase();
    expect(isId("workspace", upper)).toBe(false);
  });
});

describe("assertId", () => {
  it("returns the value it narrowed", () => {
    const value = newId("session");
    expect(assertId("session", value)).toBe(value);
  });

  it("throws naming the kind, the expected shape, and the value — never coerces", () => {
    expect(() => assertId("workspace", "5fc4b90e-55af-46cf-9938-b70f988e431d")).toThrow(
      'workspace id must match ^wsp_[0-9a-f]{32}$, got "5fc4b90e-55af-46cf-9938-b70f988e431d"'
    );
    expect(() => assertId("workspace", undefined)).toThrow(
      "workspace id must match ^wsp_[0-9a-f]{32}$, got <undefined>"
    );
  });

  it("uses the caller's label when the field name is more useful than the kind", () => {
    expect(() => assertId("workspace", "x", "x-aex-workspace-id")).toThrow(
      "x-aex-workspace-id must match ^wsp_[0-9a-f]{32}$"
    );
  });
});

describe("idPatternSource / idPattern", () => {
  it("is the ONE shape source every SQL CHECK and JSON-Schema pattern is derived from", () => {
    expect(idPatternSource("workspace")).toBe("^wsp_[0-9a-f]{32}$");
    expect(idPatternSource("resource")).toBe("^wres_[0-9a-f]{32}$");
    expect(idPattern("workspace").source).toBe(idPatternSource("workspace"));
    expect(idPattern("workspace").flags).toBe("");
  });

  it("returns a cached, anchored, flagless regex (no lastIndex hazard)", () => {
    expect(idPattern("session")).toBe(idPattern("session"));
    expect(idPattern("session").global).toBe(false);
  });
});

describe("idKindOf", () => {
  it("names the kind of a well-formed id and nothing else", () => {
    for (const kind of ID_KINDS) {
      expect(idKindOf(newId(kind))).toBe(kind);
    }
    expect(idKindOf("wsp_nope")).toBeUndefined();
    expect(idKindOf("nope_5fc4b90e55af46cf9938b70f988e431d")).toBeUndefined();
    expect(idKindOf("5fc4b90e-55af-46cf-9938-b70f988e431d")).toBeUndefined();
    expect(idKindOf(undefined)).toBeUndefined();
  });
});
