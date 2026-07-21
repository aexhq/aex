import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { CliIO } from "../src/internal.js";
import { parseStartArguments } from "../src/host/start-arguments.js";
import { resolveStartConfig } from "../src/host/start-config.js";
import { buildStartAttachments } from "../src/host/start-attachments.js";
import { buildStartSubmission } from "../src/host/start-submission.js";

const MODEL = "claude-haiku-4-5";
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

describe("start stage ownership", () => {
  it("keeps executeStartCmd a bounded orchestrator over the four start owners", () => {
    const source = readFileSync(new URL("../src/host/start-cmd.ts", import.meta.url), "utf8");
    expect(source).toContain('from "./start-arguments.js"');
    expect(source).toContain('from "./start-config.js"');
    expect(source).toContain('from "./start-attachments.js"');
    expect(source).toContain('from "./start-submission.js"');
    expect(source).not.toMatch(/\b(?:takeOptionFlag|takeBooleanFlag|collectRepeated(?:Kv|KvList)?)\(/);

    const start = source.indexOf("export async function executeStartCmd");
    const end = source.indexOf("\nasync function followAcceptedStart", start);
    expect(start).toBeGreaterThanOrEqual(0);
    expect(end).toBeGreaterThan(start);
    expect(source.slice(start, end).split("\n").length).toBeLessThanOrEqual(75);
  });
});

describe("start-specific parser", () => {
  it.each([
    ["--provider", "anthropic"],
    ["--anthropic-api-key", "secret"],
    ["--openai-api-key", "secret"],
    ["--deepseek-api-key", "secret"],
    ["--gemini-api-key", "secret"],
    ["--mistral-api-key", "secret"],
    ["--openrouter-api-key", "secret"],
    ["--doubao-api-key", "secret"],
    ["--idempotency-key", "idem"],
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
      "--provider=anthropic", "--provider", "deepseek",
      "--deepseek-api-key=first", "--deepseek-api-key", "last",
      "--model=first-model", "--model", "last-model",
      "--prompt=one", "--prompt", "two",
      "--mcp=docs=https://first.example", "--mcp", "docs=https://last.example",
      "--mcp-auth=docs=Authorization:first", "--mcp-auth", "docs=X-Key:second",
      "--metadata=mode=first", "--metadata", "mode=last",
      "--follow", "--follow"
    ]);

    expect(parsed).toMatchObject({
      explicitProvider: "deepseek",
      providerApiKeys: { deepseek: "last" },
      model: "last-model",
      prompts: ["one", "two"],
      mcpEntries: { docs: "https://last.example" },
      mcpAuthEntries: [["docs", "Authorization:first"], ["docs", "X-Key:second"]],
      metadataEntries: { mode: "last" },
      follow: true
    });
  });

  it.each([
    ["--provider"],
    ["--anthropic-api-key"],
    ["--idempotency-key"],
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
    expect(parsed).toMatchObject({ ok: false, error: expect.stringContaining("requires") });
  });

  it.each([
    [["--provider", "anthropc"], "--provider must be one of:"],
    [["--runtime-size", "tiny"], "--runtime-size must be one of:"],
    [["--runtime", "fargate"], "--runtime must be one of:"],
    [["--session-timeout", "1s"], "--session-timeout:"],
    [["--timeout", "soon"], "--timeout: invalid duration"],
    [["--mcp", "missing-equals"], "--mcp must be in the form KEY=VALUE"],
    [["--proxy-auth", "old=value"], "--proxy-endpoint and --proxy-auth are no longer supported"],
    [["position", "--unknown"], "unknown flag: --unknown"],
    [["position"], "aex start takes no positional arguments"]
  ])("preserves negative parsing for %j", (argv, error) => {
    expect(parseStartArguments(argv)).toEqual({ ok: false, error: expect.stringContaining(error) });
  });
});

describe("start config, attachment, and submission stages", () => {
  it("reads all prompt files before the system file, then resolves provider and MCP headers", async () => {
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
      "--mcp-auth", "docs=Authorization:Bearer token",
      "--anthropic-api-key", "secret"
    ]);

    const resolved = await resolveStartConfig(io, args);

    expect(resolved).toMatchObject({
      ok: true,
      value: {
        model: MODEL,
        provider: "anthropic",
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

  it("preserves config XOR, load, provider-key, and MCP diagnostics", async () => {
    const noReads = makeIo(() => {
      throw new Error("unexpected read");
    });
    await expect(resolveStartConfig(noReads, parseOk([
      "--config", "config.json", "--model", MODEL, "--anthropic-api-key", "secret"
    ]))).resolves.toEqual({
      ok: false,
      error: "--config cannot be combined with --model/--system/--prompt/--mcp/--metadata"
    });
    await expect(resolveStartConfig(noReads, parseOk([
      "--model", MODEL, "--prompt", "hello"
    ]))).resolves.toEqual({
      ok: false,
      error: expect.stringContaining("--anthropic-api-key is required")
    });
    await expect(resolveStartConfig(noReads, parseOk([
      "--model", MODEL, "--prompt", "hello", "--anthropic-api-key", "secret",
      "--mcp-auth", "missing=Authorization:token"
    ]))).resolves.toEqual({
      ok: false,
      error: "--mcp-auth missing: no matching --mcp / mcpServers entry declared"
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
      "--runtime-size=", "--session-timeout=",
      "--anthropic-api-key", "secret"
    ]);
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
