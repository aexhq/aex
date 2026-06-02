/**
 * Live scenario: live-sdk-mcp-invocation.test.ts
 *
 * Matrix test — same assertion body across (provider, runtime) cells.
 * Proves the agent ACTUALLY CALLS a remote MCP tool end-to-end:
 *   SDK → POST /runs (with mcpServers wired)
 *      → dispatcher routes (native vs managed)
 *      → runtime materializes the MCP into the agent manifest
 *      → model picks the MCP tool and the runtime emits a tool_request
 *      → runtime receives the upstream MCP response → tool_response
 *      → assistant_text echoes the answer
 *
 * Existing tests prove the wiring carries through (skill_loaded markers,
 * MCP flag count) but never prove the model used the MCP. A
 * silently-misrouted proxy URL would pass them. This test fails when the
 * model can't reach the MCP, when the proxy mishandles auth, or when the
 * adapter drops the tool_request translation.
 *
 * Per AGENTS.md's "No feature gates between runtimes" rule, this file
 * runs the SAME body on every cell:
 *   - (anthropic, native)   — Anthropic Managed Agents
 *   - (anthropic, managed)  — Goose Managed + Anthropic via provider-proxy
 *   - (deepseek,  managed)  — Goose Managed + DeepSeek via provider-proxy
 *
 * If a cell fails, the failure points at the work item that closes the
 * gap (e.g. native-MCP needs vault translation per
 * platform.claude.com/docs/en/managed-agents/mcp-connector). The test
 * never short-circuits or skips: a missing translation is a bug, not a
 * configuration toggle.
 *
 * Required env:
 *   ANTPATH_LIVE_API_BASE              live hosted API URL
 *   ANTPATH_LIVE_API_TOKEN             workspace API token
 *   ANTPATH_USER_TEST_ANTHROPIC_KEY    customer Anthropic key
 *   ANTPATH_USER_TEST_DEEPSEEK_KEY     customer DeepSeek key
 *   ANTPATH_USER_TEST_TARBALL          packed SDK tarball
 *     OR ANTPATH_USER_TEST_VERSION     published version on npm
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (mcp-invocation): required env ${name} is missing.`);
  }
  return value;
}

const liveApiBase = requireEnv("ANTPATH_LIVE_API_BASE");
const apiToken = requireEnv("ANTPATH_LIVE_API_TOKEN");
const anthropicKey = requireEnv("ANTPATH_USER_TEST_ANTHROPIC_KEY");
const deepseekKey = requireEnv("ANTPATH_USER_TEST_DEEPSEEK_KEY");
const anthropicModel = process.env["ANTPATH_USER_TEST_ANTHROPIC_MODEL"] ?? "claude-haiku-4-5";
const deepseekModel = process.env["ANTPATH_USER_TEST_DEEPSEEK_MODEL"] ?? "deepseek-chat";

// DeepWiki — public, unauthenticated MCP exposing GitHub repo Q&A tools.
// Same upstream as live-sdk-comprehensive uses; needs no credential.
const MCP_NAME = "deepwiki";
const MCP_URL = "https://mcp.deepwiki.com/mcp";

interface Cell {
  readonly id: string;
  readonly provider: "anthropic" | "deepseek";
  readonly runtime: "native" | "managed";
  readonly model: string;
  readonly keyEnvName: string;
  readonly keyValue: string;
}

const CELLS: readonly Cell[] = [
  {
    id: "anthropic-native",
    provider: "anthropic",
    runtime: "native",
    model: anthropicModel,
    keyEnvName: "ANTHROPIC_KEY_SUBMIT",
    keyValue: anthropicKey
  },
  {
    id: "anthropic-managed",
    provider: "anthropic",
    runtime: "managed",
    model: anthropicModel,
    keyEnvName: "ANTHROPIC_KEY_SUBMIT",
    keyValue: anthropicKey
  },
  {
    id: "deepseek-managed",
    provider: "deepseek",
    runtime: "managed",
    model: deepseekModel,
    keyEnvName: "DEEPSEEK_KEY_SUBMIT",
    keyValue: deepseekKey
  }
];

interface CaseResult {
  readonly runId: string;
  readonly runStatus: string;
  readonly runtime: string;
  readonly provider: string;
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly toolRequests: ReadonlyArray<{ id: string | null; name: string | null; extension: string | null }>;
  readonly toolResponses: ReadonlyArray<{ id: string | null }>;
  readonly assistantTextJoined: string;
  readonly assistantTextEventCount: number;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
  readonly leakedAnthropicKey: boolean;
  readonly leakedDeepseekKey: boolean;
}

function buildPassEnv(extras: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extras };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  if (process.platform === "win32") {
    for (const k of [
      "SystemRoot",
      "SystemDrive",
      "TEMP",
      "TMP",
      "USERPROFILE",
      "APPDATA",
      "LOCALAPPDATA",
      "ComSpec",
      "ProgramFiles",
      "ProgramData"
    ]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  } else {
    for (const k of ["HOME", "TMPDIR", "LANG", "LC_ALL"]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  }
  return env;
}

function buildScript(cell: Cell): string {
  // Prompt steers the model toward calling the MCP — a generic factual
  // question the model could likely answer without tools is risky, but
  // "use the deepwiki tool to look up …" plus an explicit reply format
  // anchors tool selection. Tool_request observation is the actual
  // evidence; the assistant_text is only a tiebreaker.
  const prompt =
    `Use the ${MCP_NAME} tool to look up the primary programming language ` +
    `of the GitHub repository anthropics/anthropic-cookbook. ` +
    `Reply with exactly one line: lang=<language>.`;
  return `
    import { AntpathClient, McpServer } from "antpath";

    const client = new AntpathClient({
      baseUrl: process.env.ANTPATH_API_BASE,
      apiToken: process.env.ANTPATH_API_TOKEN
    });

    const mcp = McpServer.remote({
      name: ${JSON.stringify(MCP_NAME)},
      url: ${JSON.stringify(MCP_URL)}
    });

    const runId = await client.submitRun({
      provider: ${JSON.stringify(cell.provider)},
      runtime: ${JSON.stringify(cell.runtime)},
      model: ${JSON.stringify(cell.model)},
      prompt: ${JSON.stringify(prompt)},
      mcpServers: [mcp],
      // builtins:[] removes the shell/edit/web fallbacks so the model
      // can ONLY satisfy the prompt via the MCP. With them present
      // both runtimes prefer the cheaper builtin shell + curl path and
      // the MCP — even when correctly wired — is never invoked. This
      // pins the assertion to MCP behaviour instead of model whim.
      builtins: [],
      secrets: { ${cell.provider}: { apiKey: process.env.${cell.keyEnvName} } },
      idempotencyKey: "mcp-invocation-${cell.id}-" + Date.now()
    });

    const deadline = Date.now() + 6 * 60_000;
    let run = null;
    while (Date.now() < deadline) {
      run = await client.getRun(runId);
      if (run.status === "succeeded" || run.status === "failed" || run.status === "cancelled") break;
      await new Promise((r) => setTimeout(r, 2_500));
    }
    if (!run || (run.status !== "succeeded" && run.status !== "failed" && run.status !== "cancelled")) {
      process.stderr.write(JSON.stringify({ kind: "timeout", run }, null, 2));
      process.exit(2);
    }

    const events = await client.listEvents(runId);

    const toolRequests = events
      .filter((e) => e.type === "TOOL_CALL_START")
      .map((e) => ({
        id: e.data && typeof e.data.id === "string" ? e.data.id : null,
        name: e.data && typeof e.data.name === "string" ? e.data.name : null,
        extension: e.data && typeof e.data.extension === "string" ? e.data.extension : null
      }));
    const toolResponses = events
      .filter((e) => e.type === "TOOL_CALL_RESULT")
      .map((e) => ({ id: e.data && typeof e.data.id === "string" ? e.data.id : null }));

    const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
    const assistantTextJoined = assistantTextEvents
      .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
      .join(" ");

    const terminal = events.find((e) => (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR"));
    const streamErrors = events
      .filter((e) => e.type === "stream_error")
      .map((e) => (e.data && typeof e.data === "object" ? e.data : { unknown: true }));

    const serialized = JSON.stringify({ run, events });
    const anthropicEnv = process.env.ANTHROPIC_KEY ?? "";
    const deepseekEnv = process.env.DEEPSEEK_KEY ?? "";
    const result = {
      runId: runId,
      runStatus: run.status,
      runtime: run.runtime ?? "(missing)",
      provider: run.provider ?? "(missing)",
      eventCount: events.length,
      eventKinds: events.map((e) => e.type),
      toolRequests,
      toolResponses,
      assistantTextJoined,
      assistantTextEventCount: assistantTextEvents.length,
      terminalKind: terminal ? terminal.type : null,
      terminalData: terminal ? terminal.data : null,
      streamErrors,
      leakedAnthropicKey: anthropicEnv.length > 0 && serialized.includes(anthropicEnv),
      leakedDeepseekKey: deepseekEnv.length > 0 && serialized.includes(deepseekEnv)
    };
    process.stdout.write(JSON.stringify(result));
  `;
}

function dumpResult(cell: Cell, result: CaseResult): string {
  const lines: string[] = [];
  lines.push(`cell=${cell.id} runId=${result.runId}`);
  lines.push(`runStatus=${result.runStatus} runtime=${result.runtime} provider=${result.provider}`);
  lines.push(`terminalKind=${result.terminalKind} terminalData=${JSON.stringify(result.terminalData)}`);
  lines.push(`eventKinds=[${result.eventKinds.join(", ")}]`);
  lines.push(
    `toolRequests=${JSON.stringify(result.toolRequests).slice(0, 600)}`
  );
  lines.push(`toolResponses=${JSON.stringify(result.toolResponses).slice(0, 400)}`);
  if (result.streamErrors.length > 0) {
    lines.push(`streamErrors:`);
    for (const se of result.streamErrors) {
      lines.push(`  - ${JSON.stringify(se).slice(0, 600)}`);
    }
  }
  lines.push(`assistantText=${result.assistantTextJoined.slice(0, 600)}`);
  return lines.join("\n");
}

async function runCell(cell: Cell, installDir: string): Promise<CaseResult> {
  const script = buildScript(cell);
  const scriptPath = join(installDir, `mcp-invocation-${cell.id}.mjs`);
  writeFileSync(scriptPath, script);
  const passEnv = buildPassEnv({
    ANTPATH_API_BASE: liveApiBase,
    ANTPATH_API_TOKEN: apiToken,
    [cell.keyEnvName]: cell.keyValue,
    ANTHROPIC_KEY: anthropicKey,
    DEEPSEEK_KEY: deepseekKey
  });
  const child = await runCommand(process.execPath, [scriptPath], {
    cwd: installDir,
    timeoutMs: 8 * 60_000,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `mcp-invocation runner (${cell.id}) exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as CaseResult;
}

let install: InstallResult;

beforeAll(async () => {
  install = await installAntpath();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("live mcp invocation — agent actually calls a remote MCP tool", () => {
  it.each(CELLS)(
    "$id: deepwiki MCP wired, model picks the tool, tool_request/tool_response emitted",
    async (cell) => {
      const result = await runCell(cell, install.installDir);
      const dump = (): string => dumpResult(cell, result);

      expect(result.runStatus, dump()).toBe("succeeded");
      expect(result.runtime).toBe(cell.runtime);
      expect(result.provider).toBe(cell.provider);

      // Event frame: runtime_started present + last event is runtime_terminal.
      // (Some runtimes emit preflight notifications before runtime_started,
      // so we don't pin position 0 — the existence of the started event is
      // what matters.)
      expect(result.eventKinds).toContain("RUN_STARTED");
      expect(result.terminalKind).toBe("RUN_FINISHED");
      // Every clean terminal MUST carry reason="complete" — both adapters
      // always populate reason on the success path. Tolerating `undefined`
      // (pre-Phase-1) was masking field-loss regressions.
      const terminalReason = result.terminalData ? result.terminalData["reason"] : undefined;
      if (terminalReason !== "complete") {
        throw new Error(`terminal reason=${terminalReason} (expected "complete")\n\n${dump()}`);
      }

      // The agent actually selected the MCP tool. Goose adapter emits
      // tool_request.data.name = "<serverName>__<toolName>" with
      // data.extension = "<serverName>" (goose-adapter.mjs:113-133).
      // Native (Anthropic) adapter emits bare tool names — adapter.ts:203-222
      // — so we accept either: name starts with the MCP name, OR
      // extension equals the MCP name.
      const mcpRequests = result.toolRequests.filter(
        (r) =>
          (r.name && r.name.startsWith(MCP_NAME)) ||
          r.extension === MCP_NAME
      );
      if (mcpRequests.length === 0) {
        throw new Error(`no tool_request matched MCP "${MCP_NAME}"\n\n${dump()}`);
      }

      // Every MCP tool_request must have a matching tool_response (by id).
      // A request without a response is the silent-MCP-failure mode this
      // test is supposed to catch.
      const responseIds = new Set(result.toolResponses.map((r) => r.id).filter(Boolean));
      const matchedRequests = mcpRequests.filter((r) => r.id && responseIds.has(r.id));
      if (matchedRequests.length === 0) {
        throw new Error(
          `no tool_response correlated to an MCP tool_request by id\n\n${dump()}`
        );
      }

      // The model used the tool's answer in its reply. Strip whitespace —
      // streaming responses fragment per-token across content blocks.
      const normalized = result.assistantTextJoined.replace(/\s+/g, "");
      if (!normalized.includes("lang=")) {
        throw new Error(`assistant_text missing "lang=" marker\n\n${dump()}`);
      }
      expect(result.assistantTextEventCount).toBeGreaterThan(0);

      // No secret leakage.
      expect(result.leakedAnthropicKey, dump()).toBe(false);
      expect(result.leakedDeepseekKey, dump()).toBe(false);
    },
    10 * 60_000
  );
});
