/**
 * Live scenario: live-sdk-builtin-tools.test.ts
 *
 * Matrix test — same assertion body across managed provider cells, with
 * a positive sub-run (builtins enabled) AND a negative sub-run
 * (builtins:[] disarmed). Proves:
 *
 *   - The agent ACTUALLY USES a built-in shell/edit tool when one is
 *     available (positive). The managed runtime surfaces this as
 *     tool_request.data.name = "shell" (runtime adapter.mjs:113-133).
 *   - When builtins:[] is requested, ZERO shell-family tool_requests fire
 *     (negative). This is the only proof the empty allowlist actually
 *     disarms tooling; the default ["developer"] would otherwise let the
 *     model reach for the shell anyway.
 *
 * Both sub-runs run on every managed provider cell.
 *
 * Required env: same as other live-sdk-* files.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (builtin-tools): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"] ?? "deepseek-chat";

// Tool names that the agent might call to satisfy "use your shell tool".
// managed runtime: "shell" (developer builtin). Older event payloads may use "bash".
const SHELL_FAMILY = new Set(["shell", "bash"]);

interface Cell {
  readonly id: string;
  readonly provider: "deepseek";
  readonly runtime: "managed";
  readonly model: string;
  readonly keyEnvName: string;
  readonly keyValue: string;
}

const CELLS: readonly Cell[] = [
  { id: "deepseek-managed",  provider: "deepseek",  runtime: "managed", model: deepseekModel,  keyEnvName: "DEEPSEEK_KEY_SUBMIT",  keyValue: deepseekKey }
];

interface CaseResult {
  readonly runId: string;
  readonly runStatus: string;
  readonly runtime: string;
  readonly provider: string;
  readonly mode: "positive" | "negative";
  readonly marker: string;
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly toolRequestNames: readonly string[];
  readonly assistantTextJoined: string;
  readonly assistantTextEventCount: number;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
  readonly leakedProviderKey: boolean;
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

function buildScript(cell: Cell, mode: "positive" | "negative", marker: string): string {
  const builtinsLiteral = mode === "positive" ? '["developer"]' : "[]";
  const prompt =
    mode === "positive"
      ? `Using your shell tool, run \`printf '${marker}\\n'\` and then reply with the exact line you printed.`
      : `Using your shell tool, run \`printf '${marker}\\n'\` and then reply with the exact line you printed. ` +
        `If you have no shell tool available, reply briefly explaining that.`;

  return `
    import { AgentExecutor } from "@aexhq/sdk";

    const client = new AgentExecutor({
      baseUrl: process.env.AEX_API_URL,
      apiToken: process.env.AEX_API_TOKEN
    });

    const runId = await client.submitRun({
      provider: ${JSON.stringify(cell.provider)},
      runtime: ${JSON.stringify(cell.runtime)},
      model: ${JSON.stringify(cell.model)},
      prompt: ${JSON.stringify(prompt)},
      builtins: ${builtinsLiteral},
      secrets: { ${cell.provider}: { apiKey: process.env.${cell.keyEnvName} } },
      idempotencyKey: "builtins-${cell.id}-${mode}-" + Date.now()
    });

    const deadline = Date.now() + 5 * 60_000;
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

    const toolRequestNames = events
      .filter((e) => e.type === "TOOL_CALL_START")
      .map((e) => (e.data && typeof e.data.name === "string" ? e.data.name : ""))
      .filter(Boolean);

    const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
    const assistantTextJoined = assistantTextEvents
      .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
      .join(" ");

    const terminal = events.find((e) => (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR"));
    const streamErrors = events
      .filter((e) => e.type === "stream_error")
      .map((e) => (e.data && typeof e.data === "object" ? e.data : { unknown: true }));

    const serialized = JSON.stringify({ run, events });
    const deepseekEnv = process.env.DEEPSEEK_KEY ?? "";
    const result = {
      runId: runId,
      runStatus: run.status,
      runtime: run.runtime ?? "(missing)",
      provider: run.provider ?? "(missing)",
      mode: ${JSON.stringify(mode)},
      marker: ${JSON.stringify(marker)},
      eventCount: events.length,
      eventKinds: events.map((e) => e.type),
      toolRequestNames,
      assistantTextJoined,
      assistantTextEventCount: assistantTextEvents.length,
      terminalKind: terminal ? terminal.type : null,
      terminalData: terminal ? terminal.data : null,
      streamErrors,
      leakedProviderKey: deepseekEnv.length > 0 && serialized.includes(deepseekEnv),
      leakedDeepseekKey: deepseekEnv.length > 0 && serialized.includes(deepseekEnv)
    };
    process.stdout.write(JSON.stringify(result));
    process.exit(0);
  `;
}

function dumpResult(cell: Cell, result: CaseResult): string {
  const lines: string[] = [];
  lines.push(`cell=${cell.id} mode=${result.mode} runId=${result.runId}`);
  lines.push(`runStatus=${result.runStatus} runtime=${result.runtime} provider=${result.provider}`);
  lines.push(`marker=${result.marker}`);
  lines.push(`terminalKind=${result.terminalKind} terminalData=${JSON.stringify(result.terminalData)}`);
  lines.push(`eventKinds=[${result.eventKinds.join(", ")}]`);
  lines.push(`toolRequestNames=[${result.toolRequestNames.join(", ")}]`);
  if (result.streamErrors.length > 0) {
    lines.push(`streamErrors:`);
    for (const se of result.streamErrors) {
      lines.push(`  - ${JSON.stringify(se).slice(0, 600)}`);
    }
  }
  lines.push(`assistantText=${result.assistantTextJoined.slice(0, 600)}`);
  return lines.join("\n");
}

async function runCell(cell: Cell, mode: "positive" | "negative", installDir: string): Promise<CaseResult> {
  const marker = `BUILTIN-OK-${Math.random().toString(36).slice(2, 10).toUpperCase()}`;
  const script = buildScript(cell, mode, marker);
  const scriptPath = join(installDir, `builtins-${cell.id}-${mode}.mjs`);
  writeFileSync(scriptPath, script);
  const passEnv = buildPassEnv({
    AEX_API_URL: apiUrl,
    AEX_API_TOKEN: apiToken,
    [cell.keyEnvName]: cell.keyValue,
    DEEPSEEK_KEY: deepseekKey
  });
  const child = await runCommand(process.execPath, [scriptPath], {
    cwd: installDir,
    timeoutMs: 7 * 60_000,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `builtins runner (${cell.id} ${mode}) exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as CaseResult;
}

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("live built-in tools — agent uses (and can be denied) shell-family tools", () => {
  it.each(CELLS)(
    "$id positive: builtins:['developer'], model calls shell, marker echoed",
    async (cell) => {
      const result = await runCell(cell, "positive", install.installDir);
      const dump = (): string => dumpResult(cell, result);

      expect(result.runStatus, dump()).toBe("succeeded");
      expect(result.runtime).toBe(cell.runtime);
      expect(result.eventKinds).toContain("RUN_STARTED");
      expect(result.terminalKind).toBe("RUN_FINISHED");
      // Every clean terminal MUST carry reason="complete" — both adapters
      // (managed DeepSeek runtime) always populate reason on the success
      // path. Tolerating `undefined` (the pre-Phase-1 pattern) was masking
      // a regression where the field could go missing entirely.
      const terminalReason = result.terminalData ? result.terminalData["reason"] : undefined;
      if (terminalReason !== "complete") {
        throw new Error(`terminal reason=${terminalReason} (expected "complete")\n\n${dump()}`);
      }

      // The agent reached for the shell. The managed runtime surfaces "shell"; older event
      // payloads may surface "bash". Accept either.
      const shellCalls = result.toolRequestNames.filter((n) => SHELL_FAMILY.has(n));
      if (shellCalls.length === 0) {
        throw new Error(
          `expected ≥1 tool_request with name ∈ {shell, bash}, got [${result.toolRequestNames.join(", ")}]\n\n${dump()}`
        );
      }

      // The model echoed the printf output back. Stripping whitespace so
      // streaming fragmentation doesn't break the match.
      const normalized = result.assistantTextJoined.replace(/\s+/g, "");
      if (!normalized.includes(result.marker)) {
        throw new Error(`assistant_text missing marker "${result.marker}"\n\n${dump()}`);
      }

      expect(result.leakedProviderKey, dump()).toBe(false);
      expect(result.leakedDeepseekKey, dump()).toBe(false);
    },
    9 * 60_000
  );

  it.each(CELLS)(
    "$id negative: builtins:[], zero shell-family tool_requests",
    async (cell) => {
      const result = await runCell(cell, "negative", install.installDir);
      const dump = (): string => dumpResult(cell, result);

      // The run must still complete cleanly — disarming tools does not
      // crash the runtime; the agent answers without tool use.
      expect(result.runStatus, dump()).toBe("succeeded");
      expect(result.eventKinds).toContain("RUN_STARTED");
      expect(result.terminalKind).toBe("RUN_FINISHED");
      // Every clean terminal MUST carry reason="complete" — both adapters
      // (managed DeepSeek runtime) always populate reason on the success
      // path. Tolerating `undefined` (the pre-Phase-1 pattern) was masking
      // a regression where the field could go missing entirely.
      const terminalReason = result.terminalData ? result.terminalData["reason"] : undefined;
      if (terminalReason !== "complete") {
        throw new Error(`terminal reason=${terminalReason} (expected "complete")\n\n${dump()}`);
      }

      // The only proof the empty allowlist actually disarmed tooling.
      const shellCalls = result.toolRequestNames.filter((n) => SHELL_FAMILY.has(n));
      if (shellCalls.length !== 0) {
        throw new Error(
          `expected ZERO shell-family tool_requests with builtins:[], got [${shellCalls.join(", ")}]\n\n${dump()}`
        );
      }

      // Sanity: the agent still produced some text (graceful "no tool"
      // reply rather than nothing).
      expect(result.assistantTextEventCount).toBeGreaterThan(0);
      expect(result.leakedProviderKey, dump()).toBe(false);
      expect(result.leakedDeepseekKey, dump()).toBe(false);
    },
    9 * 60_000
  );
});
