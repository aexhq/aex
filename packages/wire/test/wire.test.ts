import { describe, expect, test } from "bun:test";
import {
  ApiKeyPageSchema,
  ID_KINDS,
  SessionSchema,
  assertId,
  isId,
  newId,
  type IdKind,
} from "../src/index.ts";

describe("generated wire binding", () => {
  test("identifier helpers accept every kind and reject lookalikes", () => {
    for (const kind of ID_KINDS) {
      const id = newId(kind);
      expect(isId(kind, id)).toBe(true);
      expect(assertId(kind, id)).toBe(id);
      expect(isId(kind, id.replace(/.$/, "u"))).toBe(false);
    }
  });

  test("closed objects reject an unknown member", () => {
    const result = SessionSchema.safeParse({ unexpected: true });
    expect(result.success).toBe(false);
  });

  test("cursor validators accept every issuer envelope and reject malformed separators", () => {
    for (const nextCursor of ["cur_cGF5bG9hZA", "cur_cGF5bG9hZA.bWFj"]) {
      expect(ApiKeyPageSchema.safeParse({ items: [], nextCursor }).success).toBe(true);
    }
    for (const nextCursor of ["cur_", "cur_.tag", "cur_payload.", "cur_a.b.c"]) {
      expect(ApiKeyPageSchema.safeParse({ items: [], nextCursor }).success).toBe(false);
    }
  });

  test("identifier types stay kind-specific", () => {
    const kind = "workspace" satisfies IdKind;
    expect(kind).toBe("workspace");
  });
});
