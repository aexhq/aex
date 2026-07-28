import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import {
  collectRepeated,
  collectRepeatedKv,
  collectRepeatedKvList,
  extractCommonHostFlags,
  takeOptionFlag
} from "../src/host/common.js";

const here = dirname(fileURLToPath(import.meta.url));
const hostSource = resolve(here, "../src/host");

const SINGLE_VALUE_FLAGS = [
  "--webhook",
  "--runtime-size",
  "--runtime",
  "--session-timeout",
  "--timeout",
  "--signal",
  "--config",
  "--model",
  "--system",
  "--out",
  "--only",
  "--name",
  "--ext",
  "--type",
  "--content-type",
  "--interval",
  "--from",
  "--limit",
  "--since",
  "--return-url",
  "--success-url",
  "--cancel-url",
  "--threshold",
  "--amount",
  "--email",
  "--role",
  "--org"
] as const;

const REPEATED_VALUE_FLAGS = [
  "--prompt",
  "--skill",
  "--tool",
  "--instructions",
  "--file",
  "--filter",
  "--proxy-endpoint"
] as const;

const REPEATED_KV_FLAGS = ["--mcp", "--metadata", "--proxy-auth"] as const;

describe("value-taking option syntax", () => {
  it.each([...SINGLE_VALUE_FLAGS])("gives %s identical split and equals consumption", (flag) => {
    const split = takeOptionFlag(["before", flag, "value=with=equals", "after"], flag);
    const joined = takeOptionFlag(["before", `${flag}=value=with=equals`, "after"], flag);
    expect(joined).toEqual(split);
    expect(split).toEqual({ value: "value=with=equals", remaining: ["before", "after"], error: null });
  });

  it("preserves single-value duplicate, ownership, empty-value, and flag-like-value behavior", () => {
    expect(takeOptionFlag(["--timeout=1s", "x", "--timeout", "2s"], "--timeout")).toEqual({
      value: "2s",
      remaining: ["x"],
      error: null
    });
    expect(takeOptionFlag(["--timeout-ms=1s", "--timeoutish=2s"], "--timeout")).toEqual({
      value: undefined,
      remaining: ["--timeout-ms=1s", "--timeoutish=2s"],
      error: null
    });
    expect(takeOptionFlag(["--timeout="], "--timeout")).toEqual({ value: "", remaining: [], error: null });
    expect(takeOptionFlag(["--timeout", "--other=value"], "--timeout")).toEqual({
      value: "--other=value",
      remaining: [],
      error: null
    });
    expect(takeOptionFlag(["--", "--timeout=1s"], "--timeout")).toEqual({
      value: "1s",
      remaining: ["--"],
      error: null
    });
    expect(takeOptionFlag(["x", "--timeout"], "--timeout")).toEqual({
      value: undefined,
      remaining: ["x"],
      error: "--timeout requires a value"
    });
  });

  it.each([...REPEATED_VALUE_FLAGS])("gives repeatable %s identical split and equals consumption", (flag) => {
    const split = collectRepeated([flag, "one", "middle", flag, "two=2"], flag);
    const joined = collectRepeated([`${flag}=one`, "middle", `${flag}=two=2`], flag);
    expect(joined).toEqual(split);
    expect(split).toEqual({ values: ["one", "two=2"], remaining: ["middle"], error: null });
  });

  it("preserves repeatable-value order, ownership, empty values, missing-value errors, and -- handling", () => {
    expect(collectRepeated(["--prompt=", "--promptish=x", "--", "--prompt=last"], "--prompt")).toEqual({
      values: ["", "last"],
      remaining: ["--promptish=x", "--"],
      error: null
    });
    expect(collectRepeated(["before", "--prompt"], "--prompt")).toEqual({
      values: [],
      remaining: ["before"],
      error: "--prompt requires a value"
    });
  });

  it.each([...REPEATED_KV_FLAGS])("gives repeatable key/value %s identical split and equals consumption", (flag) => {
    const split = collectRepeatedKv([flag, "a=one", "middle", flag, "a=two=2"], flag);
    const joined = collectRepeatedKv([`${flag}=a=one`, "middle", `${flag}=a=two=2`], flag);
    expect(joined).toEqual(split);
    expect(split).toEqual({ entries: { a: "two=2" }, remaining: ["middle"], error: null });
  });

  it("preserves key/value ownership and malformed-value diagnostics", () => {
    expect(collectRepeatedKv(["--metadataish=a=b", "--metadata=a=1"], "--metadata")).toEqual({
      entries: { a: "1" },
      remaining: ["--metadataish=a=b"],
      error: null
    });
    expect(collectRepeatedKv(["before", "--metadata="], "--metadata")).toEqual({
      entries: {},
      remaining: ["before"],
      error: "--metadata must be in the form KEY=VALUE (got: )"
    });
    expect(collectRepeatedKv(["before", "--metadata"], "--metadata")).toEqual({
      entries: {},
      remaining: ["before"],
      error: "--metadata requires a KEY=VALUE argument"
    });
  });

  it("gives duplicate-preserving key/value lists split/equals parity", () => {
    const split = collectRepeatedKvList([
      "--mcp-auth", "github=Authorization:Bearer one",
      "middle",
      "--mcp-auth", "github=X-Key:two"
    ], "--mcp-auth");
    const joined = collectRepeatedKvList([
      "--mcp-auth=github=Authorization:Bearer one",
      "middle",
      "--mcp-auth=github=X-Key:two"
    ], "--mcp-auth");
    expect(joined).toEqual(split);
    expect(split).toEqual({
      entries: [["github", "Authorization:Bearer one"], ["github", "X-Key:two"]],
      remaining: ["middle"],
      error: null
    });
  });
});

describe("common value-taking option syntax", () => {
  it("gives --api-key and --aex-url split/equals parity without exposing the key", () => {
    const split = extractCommonHostFlags([
      "status", "session-1", "--api-key", "secret-test-value", "--aex-url", "https://api.example/"
    ]);
    const joined = extractCommonHostFlags([
      "status", "session-1", "--api-key=secret-test-value", "--aex-url=https://api.example/"
    ]);
    expect(joined).toEqual(split);
    expect(joined).toEqual({
      ok: true,
      flags: {
        apiKey: "secret-test-value",
        aexUrl: "https://api.example/",
        debug: false,
        json: false,
        rest: ["status", "session-1"]
      }
    });
  });

  it("preserves mixed duplicate precedence, near-prefix ownership, empty values, and -- handling", () => {
    expect(extractCommonHostFlags([
      "--api-key=first", "--api-key", "second", "--aex-urlish=x", "--", "--aex-url=https://last.example"
    ])).toEqual({
      ok: true,
      flags: {
        apiKey: "second",
        aexUrl: "https://last.example",
        debug: false,
        json: false,
        rest: ["--aex-urlish=x", "--"]
      }
    });
    expect(extractCommonHostFlags(["--api-key=", "--aex-url="])).toEqual({
      ok: true,
      flags: { apiKey: "", aexUrl: "", debug: false, json: false, rest: [] }
    });
    expect(extractCommonHostFlags(["x", "--api-key"])).toEqual({
      ok: false,
      reason: "--api-key requires a value"
    });
    expect(extractCommonHostFlags(["--api-key", "--json"])).toEqual({
      ok: true,
      flags: { apiKey: "--json", aexUrl: null, debug: false, json: false, rest: [] }
    });
  });
});

describe("value-parser ownership", () => {
  it("has no split-only helper or command-local value consumption", () => {
    const source = readHostSources(hostSource);
    const commandSource = readHostSources(hostSource, "common.ts");
    const startSource = readFileSync(join(hostSource, "start-arguments.ts"), "utf8");
    expect(source).not.toContain("takeFlagValue");
    expect(commandSource).not.toMatch(/argv\[\+\+i\]|rest\[\+\+i\]|args\[\+\+i\]/);

    expect(sortedUnique([
      ...literalFlagArguments(source, "takeOptionFlag"),
      ...literalSingleFlagArguments(startSource, "option")
    ])).toEqual(
      sortedUnique(SINGLE_VALUE_FLAGS)
    );
    expect(startSource).toContain("takeOptionFlag(state.rest, flag)");
    expect(sortedUnique([
      ...literalFlagArguments(source, "collectRepeated"),
      ...literalSingleFlagArguments(startSource, "repeated")
    ])).toEqual(sortedUnique(REPEATED_VALUE_FLAGS));
    expect(sortedUnique([
      ...literalFlagArguments(source, "collectRepeatedKv"),
      ...literalSingleFlagArguments(startSource, "repeatedKv")
    ])).toEqual(sortedUnique(REPEATED_KV_FLAGS));
    expect(sortedUnique([
      ...literalFlagArguments(source, "collectRepeatedKvList"),
      ...literalSingleFlagArguments(startSource, "repeatedKvList")
    ])).toEqual(["--mcp-auth"]);
  });
});

function readHostSources(dir: string, excludedName?: string): string {
  return readdirSync(dir, { withFileTypes: true })
    .flatMap((entry) => entry.isDirectory()
      ? [readHostSources(join(dir, entry.name), excludedName)]
      : entry.name.endsWith(".ts") && entry.name !== excludedName
        ? [readFileSync(join(dir, entry.name), "utf8")]
        : [])
    .join("\n");
}

function literalFlagArguments(source: string, functionName: string): string[] {
  const pattern = new RegExp(`${functionName}\\([^,\\n]+,\\s*"([^"]+)"\\)`, "g");
  return sortedUnique([...source.matchAll(pattern)].map((match) => match[1]!));
}

function literalSingleFlagArguments(source: string, functionName: string): string[] {
  const pattern = new RegExp(`${functionName}\\("([^"]+)"\\)`, "g");
  return sortedUnique([...source.matchAll(pattern)].map((match) => match[1]!));
}

function sortedUnique(values: readonly string[]): string[] {
  return [...new Set(values)].sort();
}
