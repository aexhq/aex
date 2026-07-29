import { readFileSync } from "node:fs";
import { describe, expect, it } from "bun:test";
import ts from "typescript";
import type { CliIO } from "../src/internal.js";
import { parseStartArguments } from "../src/host/start-arguments.js";
import { resolveStartConfig } from "../src/host/start-config.js";
import { buildStartAttachments } from "../src/host/start-attachments.js";
import { buildStartSubmission } from "../src/host/start-submission.js";

const MODEL = "anthropic/claude-haiku-4-5";
const SKILL = "---\nname: staged-skill\ndescription: staged skill\n---\nUse stages.\n";

function parseOk(argv: readonly string[]) {
  const parsed = parseStartArguments(argv);
  expect(parsed.ok, parsed.ok ? undefined : parsed.error).toBe(true);
  if (!parsed.ok) throw new Error(parsed.error);
  return parsed.value;
}

function makeIo(read: (path: string, bytes: boolean) => string | Uint8Array | Promise<string | Uint8Array>): CliIO {
  return {
    argv: [],
    cwd: () => "C:\\start-stage",
    readFile: async (path) => String(await read(path, false)),
    readFileBytes: async (path) => {
      const value = await read(path, true);
      return typeof value === "string" ? new TextEncoder().encode(value) : value;
    },
    writeFile: async () => undefined,
    fetchImpl: fetch,
    stdout: () => undefined,
    stderr: () => undefined,
    exit: () => undefined
  };
}

function descendants(node: ts.Node): readonly ts.Node[] {
  const out: ts.Node[] = [];
  const visit = (current: ts.Node): void => {
    out.push(current);
    current.forEachChild(visit);
  };
  visit(node);
  return out;
}

function namedImports(source: ts.SourceFile, moduleName: string): readonly string[] {
  return source.statements
    .filter(ts.isImportDeclaration)
    .filter((statement) => ts.isStringLiteral(statement.moduleSpecifier) && statement.moduleSpecifier.text === moduleName)
    .flatMap((statement) => {
      const bindings = statement.importClause?.namedBindings;
      return bindings && ts.isNamedImports(bindings) ? bindings.elements.map((element) => element.name.text) : [];
    });
}

function directCallNames(node: ts.Node): readonly string[] {
  return descendants(node)
    .filter(ts.isCallExpression)
    .map((call) => ts.isIdentifier(call.expression) ? call.expression.text : undefined)
    .filter((name): name is string => name !== undefined);
}

describe("start stage ownership", () => {
  it("keeps executeStartCmd a bounded orchestrator over the four start owners", () => {
    const source = ts.createSourceFile(
      "start-cmd.ts",
      readFileSync(new URL("../src/host/start-cmd.ts", import.meta.url), "utf8"),
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TS
    );
    const execute = source.statements.find((statement) =>
      ts.isFunctionDeclaration(statement) && statement.name?.text === "executeStartCmd"
    );
    expect(execute && ts.isFunctionDeclaration(execute) ? execute.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.ExportKeyword) : false).toBe(true);
    expect(execute && ts.isFunctionDeclaration(execute) ? execute.modifiers?.some((modifier) => modifier.kind === ts.SyntaxKind.AsyncKeyword) : false).toBe(true);
    for (const [moduleName, owner] of [
      ["./start-arguments.js", "parseStartArguments"],
      ["./start-config.js", "resolveStartConfig"],
      ["./start-attachments.js", "buildStartAttachments"],
      ["./start-submission.js", "buildStartSubmission"],
      ["./start-submit.js", "submitCliRun"]
    ] as const) {
      expect(namedImports(source, moduleName), `${owner} must remain a stage dependency`).toContain(owner);
      expect(execute && ts.isFunctionDeclaration(execute) ? directCallNames(execute) : [], `${owner} must be orchestrated directly`).toContain(owner);
    }
    expect(execute && ts.isFunctionDeclaration(execute) ? directCallNames(execute) : []).not.toEqual(
      expect.arrayContaining(["takeOptionFlag", "takeBooleanFlag", "collectRepeatedKv", "collectRepeatedKvList"])
    );
  });
});

describe("start-specific parser", () => {
  it.each([
    ["--webhook", "https://hooks.example.test/aex"],
    ["--runtime-size", "0.25cpu-1gb"],
    ["--runtime", "lambda"],
    ["--session-timeout", "2m"],
    ["--timeout", "1.5s"],
    ["--config", "session.json"],
    ["--model", MODEL],
    ["--system", "system"]
  ])("normalizes split and equals syntax for %s", (flag, value) => {
    expect(parseStartArguments([flag, value])).toEqual(parseStartArguments([`${flag}=${value}`]));
  });

  it.each([
    ["--prompt", "hello"],
    ["--skill", "@skill.md"],
    ["--tool", "@tool.js"],
    ["--instructions", "@instructions.md"],
    ["--file", "@file.bin"],
    ["--proxy-endpoint", "legacy"]
  ])("normalizes repeated split and equals syntax for %s", (flag, value) => {
    expect(parseStartArguments([flag, value, flag, `${value}-2`]))
      .toEqual(parseStartArguments([`${flag}=${value}`, `${flag}=${value}-2`]));
  });

  it.each([
    ["--mcp", "docs=https://mcp.example.test"],
    ["--mcp-auth", "docs=Authorization:Bearer token"],
    ["--metadata", "mode=test"],
    ["--proxy-auth", "legacy=value"]
  ])("normalizes key/value split and equals syntax for %s", (flag, value) => {
    expect(parseStartArguments([flag, value])).toEqual(parseStartArguments([`${flag}=${value}`]));
  });

  it("preserves every duplicate-precedence family", () => {
    const parsed = parseOk([
      "--model=first-model", "--model", "last-model",
      "--prompt=one", "--prompt", "two",
      "--mcp=docs=https://first.example", "--mcp", "docs=https://last.example",
      "--mcp-auth=docs=Authorization:first", "--mcp-auth", "docs=X-Key:second",
      "--metadata=mode=first", "--metadata", "mode=last",
      "--follow", "--follow"
    ]);

    expect(parsed).toMatchObject({
      model: "last-model",
      prompts: ["one", "two"],
      mcpEntries: { docs: "https://last.example" },
      mcpAuthEntries: [["docs", "Authorization:first"], ["docs", "X-Key:second"]],
      metadataEntries: { mode: "last" },
      follow: true
    });
  });

  it.each([
    ["--webhook"],
    ["--runtime-size"],
    ["--runtime"],
    ["--session-timeout"],
    ["--timeout"],
    ["--config"],
    ["--model"],
    ["--system"],
    ["--prompt"],
    ["--skill"],
    ["--tool"],
    ["--instructions"],
    ["--file"],
    ["--mcp"],
    ["--mcp-auth"],
    ["--metadata"],
    ["--proxy-endpoint"],
    ["--proxy-auth"]
  ])("rejects a missing value for %s", (flag) => {
    const parsed = parseStartArguments([flag]);
    // Field-wise asserts instead of toMatchObject: bun's toMatchObject writes
    // asymmetric matchers back into the received object, which would replace
    // parsed.error with the matcher before the prefix assertion below reads it.
    expect(parsed.ok).toBe(false);
    if (!parsed.ok) expect(parsed.error).toContain("requires");
    if (!parsed.ok) expect(parsed.error).toMatch(new RegExp(`^aex start ${flag.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}:`));
  });

  it.each([
    [["--runtime-size", "tiny"], "aex start --runtime-size: must be one of:"],
    [["--runtime", "fargate"], "aex start --runtime: must be one of:"],
    [["--session-timeout", "1s"], "aex start --session-timeout:"],
    [["--timeout", "soon"], "aex start --timeout: invalid duration"],
    [["--mcp", "missing-equals"], "aex start --mcp: must be in the form KEY=VALUE"],
    [["--proxy-auth", "old=value"], "aex start --proxy-auth: is no longer supported"],
    [["position", "--unknown"], "aex start --unknown: unknown flag"],
    [["position"], "aex start: takes no positional arguments"]
  ])("preserves negative parsing for %j", (argv, error) => {
    expect(parseStartArguments(argv)).toEqual({ ok: false, error: expect.stringContaining(error) });
  });
});

describe("start config, attachment, and submission stages", () => {
  it("reads all prompt files before the system file, then resolves the model and MCP headers", async () => {
    const reads: string[] = [];
    const io = makeIo(async (path) => {
      reads.push(path.replaceAll("\\", "/").split("/").at(-1)!);
      return path.endsWith("p1.txt") ? "one" : path.endsWith("p2.txt") ? "two" : "system";
    });
    const args = parseOk([
      "--model", MODEL,
      "--prompt", "@p1.txt", "--prompt=@p2.txt",
      "--system", "@system.txt",
      "--mcp", "docs=https://mcp.example.test",
      "--mcp-auth", "docs=Authorization:Bearer token",]);

    const resolved = await resolveStartConfig(io, args);

    expect(resolved).toMatchObject({
      ok: true,
      value: {
        model: MODEL,
        message: ["one", "two"],
        system: "system",
        mcpServers: [{
          name: "docs",
          url: "https://mcp.example.test",
          headers: { Authorization: "Bearer token" }
        }]
      }
    });
    expect(reads).toEqual(["p1.txt", "p2.txt", "system.txt"]);
  });

  it("preserves config XOR, load, and MCP diagnostics", async () => {
    const noReads = makeIo(() => {
      throw new Error("unexpected read");
    });
    await expect(resolveStartConfig(noReads, parseOk([
      "--config", "config.json", "--model", MODEL
    ]))).resolves.toEqual({
      ok: false,
      error: "aex start --config: cannot be combined with --model/--system/--prompt/--mcp/--metadata"
    });
    await expect(resolveStartConfig(noReads, parseOk([
      "--model", MODEL, "--prompt", "hello",
      "--mcp-auth", "missing=Authorization:token"
    ]))).resolves.toEqual({
      ok: false,
      error: "aex start --mcp-auth: missing: no matching --mcp / mcpServers entry declared"
    });
  });

  it("builds attachment groups in skills/tools/instructions/files order and stops after failure", async () => {
    const reads: string[] = [];
    const io = makeIo((path, bytes) => {
      const name = path.replaceAll("\\", "/").split("/").at(-1)!;
      reads.push(`${bytes ? "bytes" : "text"}:${name}`);
      if (name === "skill.md") return SKILL;
      if (name === "tool.js") return "export default async () => ({ ok: true });\n";
      if (name === "instructions.md") return "Be concise.\n";
      return new Uint8Array([0, 1, 2]);
    });
    const args = parseOk([
      "--skill", "@skill.md", "--tool", "@tool.js",
      "--instructions", "@instructions.md", "--file", "@data.bin"
    ]);

    const attachments = await buildStartAttachments(io, args);

    expect(attachments).toMatchObject({ skills: [{ name: "staged-skill" }], tools: [{}], instructions: [{}], files: [{}] });
    expect(reads).toEqual([
      "text:skill.md", "text:tool.js", "text:instructions.md", "bytes:data.bin"
    ]);

    const failedReads: string[] = [];
    const failingIo = makeIo((path) => {
      failedReads.push(path);
      throw new Error("missing skill");
    });
    await expect(buildStartAttachments(failingIo, args)).rejects.toThrow("missing skill");
    expect(failedReads).toHaveLength(1);
    expect(failedReads[0]).toContain("skill.md");
  });

  it("keeps explicit empty runtime-size and session-timeout ahead of config values", async () => {
    const config = JSON.stringify({
      model: MODEL,
      prompt: "hello",
      runtimeSize: "1cpu-4gb",
      timeout: "5m"
    });
    const io = makeIo(() => config);
    const args = parseOk([
      "--config", "config.json",
      "--runtime-size=", "--session-timeout=",]);
    const resolved = await resolveStartConfig(io, args);
    if (!resolved.ok) throw new Error(resolved.error);
    const options = buildStartSubmission(args, resolved.value, {
      skills: [], tools: [], instructions: [], files: []
    });

    expect(options.runtime).toBeUndefined();
    expect(options.overrides).toEqual({ idleTtl: "3m" });
    expect(options).not.toHaveProperty("webhook");
    expect(options).not.toHaveProperty("idempotencyKey");
  });
});
