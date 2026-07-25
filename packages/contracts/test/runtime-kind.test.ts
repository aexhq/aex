import { describe, expect, it } from "bun:test";
import {
  DEFAULT_RUNTIME_KIND,
  RUNTIME_KINDS,
  RuntimeKinds,
  parseRuntimeKind,
  type RuntimeKind
} from "../src/runtime-kind.js";

describe("runtime-kind constants", () => {
  it("exposes the three product runtimes as the closed set", () => {
    expect(RUNTIME_KINDS).toEqual(["container", "spot_container", "lambda"]);
  });

  it("defaults to Fargate Spot — the kind that can actually execute tools today", () => {
    expect(DEFAULT_RUNTIME_KIND).toBe("spot_container");
    expect((RUNTIME_KINDS as readonly string[]).includes(DEFAULT_RUNTIME_KIND)).toBe(true);
    // `lambda` can finish an LLM turn but cannot execute a single tool call, so it
    // must never be what an omitted `runtimeKind` resolves to.
    expect(DEFAULT_RUNTIME_KIND).not.toBe("lambda");
  });

  it("keeps the symbol accessors in lockstep with the token set", () => {
    // Every RuntimeKinds value is a valid token, and every token has an accessor.
    const values = Object.values(RuntimeKinds);
    expect(new Set(values)).toEqual(new Set(RUNTIME_KINDS));
    expect(RuntimeKinds.CONTAINER).toBe("container");
    expect(RuntimeKinds.SPOT_CONTAINER).toBe("spot_container");
    expect(RuntimeKinds.LAMBDA).toBe("lambda");
  });
});

describe("parseRuntimeKind", () => {
  it("returns undefined when omitted (consumer applies the default)", () => {
    expect(parseRuntimeKind(undefined)).toBeUndefined();
  });

  it("accepts each valid token", () => {
    for (const kind of RUNTIME_KINDS) {
      expect(parseRuntimeKind(kind)).toBe(kind);
    }
  });

  it("rejects an unknown string", () => {
    expect(() => parseRuntimeKind("fargate")).toThrow(
      /runtimeKind must be one of: container, spot_container, lambda/
    );
  });

  it("rejects a non-string value", () => {
    expect(() => parseRuntimeKind(1)).toThrow(/runtimeKind must be one of/);
    expect(() => parseRuntimeKind(null)).toThrow(/runtimeKind must be one of/);
    expect(() => parseRuntimeKind({})).toThrow(/runtimeKind must be one of/);
  });

  it("does not coerce a mismatched-case token", () => {
    expect(() => parseRuntimeKind("Container")).toThrow(/runtimeKind must be one of/);
  });

  it("narrows to the RuntimeKind type on success", () => {
    const parsed: RuntimeKind | undefined = parseRuntimeKind("lambda");
    expect(parsed).toBe("lambda");
  });
});
