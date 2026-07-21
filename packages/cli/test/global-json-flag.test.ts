import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import fc from "fast-check";
import { describe, expect, it } from "vitest";
import { executeCli } from "../src/main.js";
import type { CliIO } from "../src/internal.js";
import { extractGlobalFlags, prepareHostCommand } from "../src/host/common.js";

const here = dirname(fileURLToPath(import.meta.url));
const hostSource = resolve(here, "../src/host");

const DISCOVERY_VERBS = ["models", "providers", "tools", "runtime-sizes"] as const;
const DATA_VERBS = [
  "start", "status", "deliveries", "wait", "events", "tail", "inspect", "files",
  "download", "cancel", "delete", "delete-asset", "sessions", "whoami", "billing", "webhooks"
] as const;
const CONTROL_VERBS = ["orgs", "workspaces", "keys"] as const;

function makeIo(argv: readonly string[]): {
  readonly io: CliIO;
  readonly stdout: () => string;
  readonly stderr: () => string;
  readonly exit: () => number | null;
  readonly reads: () => number;
  readonly configReads: () => number;
  readonly fetches: () => number;
} {
  let stdout = "";
  let stderr = "";
  let exit: number | null = null;
  let reads = 0;
  let configReads = 0;
  let fetches = 0;
  const io: CliIO = {
    argv: ["bun", "/aex/aex", ...argv],
    readFile: async () => {
      reads++;
      throw Object.assign(new Error("not found"), { code: "ENOENT" });
    },
    writeFile: async () => {},
    cwd: () => "/tmp",
    fetchImpl: async () => {
      fetches++;
      throw new Error("network must not be reached");
    },
    stdout: (chunk) => { stdout += chunk; },
    stderr: (chunk) => { stderr += chunk; },
    exit: (code) => { exit = code; },
    configStore: {
      location: () => "/tmp/config.json",
      read: async () => {
        configReads++;
        return null;
      },
      write: async () => {},
      clear: async () => {}
    }
  };
  return {
    io,
    stdout: () => stdout,
    stderr: () => stderr,
    exit: () => exit,
    reads: () => reads,
    configReads: () => configReads,
    fetches: () => fetches
  };
}

describe("global --json extraction", () => {
  it("owns exact occurrences, duplicates, near-prefixes, equals-like tokens, negatives, and --", () => {
    expect(extractGlobalFlags([])).toEqual({ json: false, rest: [] });
    expect(extractGlobalFlags([
      "--jsonish", "--json", "-1", "--json=true", "--", "--json", "tail"
    ])).toEqual({
      json: true,
      rest: ["--jsonish", "-1", "--json=true", "--", "tail"]
    });
  });

  it("property: removes every exact --json token and preserves every other token in order", () => {
    const nonJsonToken = fc.string().filter((token) => token !== "--json");
    const token = fc.oneof(nonJsonToken, fc.constant("--json"));
    fc.assert(fc.property(fc.array(token, { maxLength: 40 }), (argv) => {
      const extracted = extractGlobalFlags(argv);
      expect(extracted.json).toBe(argv.includes("--json"));
      expect(extracted.rest).toEqual(argv.filter((arg) => arg !== "--json"));
    }), { numRuns: 100 });
  });

  for (const verb of DISCOVERY_VERBS) {
    it.each([
      ["omitted list, leading", ["--json"]],
      ["before optional list", ["--json", "list"]],
      ["after optional list", ["list", "--json"]],
      ["duplicate around optional list", ["--json", "list", "--json"]]
    ] as const)(`${verb}: emits JSON with %s position`, async (_label, tail) => {
      const cap = makeIo([verb, ...tail]);
      await executeCli(cap.io);
      expect(cap.exit()).toBe(0);
      expect(Array.isArray(JSON.parse(cap.stdout()))).toBe(true);
      expect(cap.stderr()).toBe("");
      expect(cap.reads()).toBe(0);
      expect(cap.configReads()).toBe(0);
      expect(cap.fetches()).toBe(0);
    });

    it.each([
      { label: "omitted list", tail: [] },
      { label: "explicit list", tail: ["list"] }
    ] as const)(`${verb}: preserves human output without --json ($label)`, async ({ tail }) => {
      const cap = makeIo([verb, ...tail]);
      await executeCli(cap.io);
      expect(cap.exit()).toBe(0);
      expect(() => JSON.parse(cap.stdout())).toThrow();
      expect(cap.stderr()).toBe("");
      expect(cap.fetches()).toBe(0);
    });

    it.each([
      ["--jsonish", `unknown flag: --jsonish\nusage: aex ${verb} list [--json]\n`],
      ["--json=true", `unknown flag: --json=true\nusage: aex ${verb} list [--json]\n`],
      ["-1", `unexpected arguments: -1\nusage: aex ${verb} list [--json]\n`],
      ["--", `unknown flag: --\nusage: aex ${verb} list [--json]\n`]
    ] as const)(`${verb}: leaves %s to discovery's existing unknown owner`, async (arg, diagnostic) => {
      const cap = makeIo([verb, arg, "--json"]);
      await executeCli(cap.io);
      expect(cap.exit()).toBe(2);
      expect(cap.stdout()).toBe("");
      expect(cap.stderr()).toBe(diagnostic);
      expect(cap.reads()).toBe(0);
      expect(cap.configReads()).toBe(0);
      expect(cap.fetches()).toBe(0);
    });
  }

  it("gives every authenticated verb the same positional and duplicate extraction before business parsing", async () => {
    for (const [verb, auth] of [
      ...DATA_VERBS.map((verb) => [verb, "data"] as const),
      ...CONTROL_VERBS.map((verb) => [verb, "control"] as const)
    ]) {
      for (const argv of [
        ["--json", "left", "right"],
        ["left", "--json", "right"],
        ["left", "right", "--json"],
        ["--json", "left", "--json", "right", "--json"]
      ]) {
        const cap = makeIo([]);
        const prepared = await prepareHostCommand(
          cap.io,
          ["--api-key=test-token", ...argv],
          { verb, auth }
        );
        expect(prepared.ok, verb).toBe(true);
        if (!prepared.ok) continue;
        expect(prepared.auth, verb).toBe(auth);
        expect(prepared.flags.json, verb).toBe(true);
        expect(prepared.rest, verb).toEqual(["left", "right"]);
        expect(cap.configReads(), verb).toBe(auth === "control" ? 1 : 0);
        expect(cap.fetches(), verb).toBe(0);
      }
    }
  });

  it("has one production global extraction owner shared by auth and discovery", () => {
    const common = readFileSync(join(hostSource, "common.ts"), "utf8");
    const discovery = readFileSync(join(hostSource, "discover-cmd.ts"), "utf8");
    const commonExtractor = common.slice(
      common.indexOf("export function extractCommonHostFlags"),
      common.indexOf("export function rejectUnknownFlags")
    );
    const otherHostSource = readdirSync(hostSource, { withFileTypes: true })
      .filter((entry) => entry.isFile() && entry.name.endsWith(".ts") && !["common.ts", "discover-cmd.ts"].includes(entry.name))
      .map((entry) => readFileSync(join(hostSource, entry.name), "utf8"))
      .join("\n");

    expect(common).toMatch(/export function extractGlobalFlags\s*\(/);
    expect(commonExtractor).toContain("extractGlobalFlags(rest)");
    expect(discovery).toContain("extractGlobalFlags");
    expect(discovery).not.toMatch(/wantsJson|takeBooleanFlag\([^)]*["']--json["']/);
    expect(otherHostSource).not.toContain("extractGlobalFlags");
  });
});
