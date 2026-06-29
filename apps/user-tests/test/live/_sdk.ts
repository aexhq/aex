/**
 * Shared scaffolding for the config-fix USER tests (SDK-driven, customer
 * perspective). These differ from raw API probes; THESE drive the
 * installed `aex` SDK end-to-end
 * (SDK → /runs → runtime → events), the real customer surface.
 *
 * Each test installs the SDK (install fixture), then runs a small Bun script
 * IN the install dir that builds a submission via the SDK's classes
 * (AgentExecutor/AgentsMd/ProxyEndpoint/…), submits, polls to terminal, and
 * prints a standard result JSON which the test asserts on.
 *
 * They validate the FIXED behaviour and so only pass once the fixes are
 * DEPLOYED to the remote hosted API. Env mirrors the other user-tests:
 *   AEX_API_URL, AEX_API_TOKEN,
 *   DEEPSEEK_API_KEY,
 *   AEX_USER_TEST_TARBALL | AEX_USER_TEST_VERSION (the SDK to install).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { getBunCommand, runCommand, type InstallResult } from "../_fixtures/install.js";

export interface UserEnv {
  readonly apiBase: string;
  readonly apiToken: string;
  readonly deepseekKey?: string;
  readonly deepseekModel: string;
}

function req(name: string): string {
  const v = process.env[name];
  if (!v || v.length === 0) {
    throw new Error(
      `user-tests: required env ${name} is missing. The SDK config-fix tests run against a real aex-local deploy with a real provider key.`
    );
  }
  return v;
}

export function requireUserEnv(opts: { deepseek?: boolean } = {}): UserEnv {
  const env: UserEnv = {
    apiBase: req("AEX_API_URL").replace(/\/$/, ""),
    apiToken: req("AEX_API_TOKEN"),
    deepseekModel: process.env.AEX_USER_TEST_DEEPSEEK_MODEL ?? "deepseek-chat"
  };
  return {
    ...env,
    ...(opts.deepseek ? { deepseekKey: req("DEEPSEEK_API_KEY") } : {})
  };
}

/** Whitespace-stripped text — streamed assistant events fragment tokens. */
export function dense(s: string): string {
  return s.replace(/\s+/g, "");
}

export function observedRunText(result: SdkRunResult): string {
  return dense([result.assistantText, result.toolResultText].join(" "));
}

export function runDiagnostics(result: SdkRunResult): string {
  return [
    `runId=${result.runId}`,
    `status=${result.status}`,
    `runtime=${result.runtime ?? "(none)"}`,
    `provider=${result.provider ?? "(none)"}`,
    `terminalKind=${result.terminalKind ?? "(none)"}`,
    `events=[${result.eventKinds.join(", ")}]`,
    `assistantText=${JSON.stringify(result.assistantText).slice(0, 500)}`,
    `toolResultText=${JSON.stringify(result.toolResultText).slice(0, 500)}`,
    `streamErrors=${JSON.stringify(result.streamErrors).slice(0, 500)}`,
    `outputCount=${result.outputCount}`
  ].join("\n");
}

export interface SdkRunResult {
  readonly runId: string;
  readonly status: string;
  readonly runtime: string | null;
  readonly provider: string | null;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly assistantText: string;
  readonly toolResultText: string;
  readonly eventKinds: readonly string[];
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
  readonly outputCount: number;
}

/**
 * Script preamble: imports the SDK + builds the client from env. Available
 * in-script: `client`, `DEEPSEEK_KEY`, `MODEL_DEEPSEEK`, and the classes
 * `AgentsMd` / `ProxyEndpoint`.
 */
const PREAMBLE = `
import { AgentExecutor, AgentsMd, ProxyEndpoint } from "@aexhq/sdk";
const client = new AgentExecutor({ baseUrl: process.env.AEX_API_URL, apiToken: process.env.AEX_API_TOKEN });
const DEEPSEEK_KEY = process.env.DEEPSEEK_KEY;
const MODEL_DEEPSEEK = process.env.MODEL_DEEPSEEK;
`;

/** Poll-to-terminal + read events/outputs + print the standard result JSON. */
const TAIL = `
const deadline = Date.now() + Number(process.env.WAIT_MS || "240000");
let run = null;
while (Date.now() < deadline) {
  run = await client.getRun(runId);
  if (["succeeded","failed","cancelled","timed_out"].includes(run.status)) break;
  await new Promise((r) => setTimeout(r, 2000));
}
const events = await client.listEvents(runId).catch(() => []);
let outputs = [];
try { outputs = await client.listOutputs(runId); } catch {}
const text = events
  .filter((e) => e.type === "TEXT_MESSAGE_CONTENT")
  .map((e) => (e && e.data && typeof e.data.text === "string" ? e.data.text : ""))
  .join(" ");
const toolResultText = events
  .filter((e) => e.type === "TOOL_CALL_RESULT")
  .map((e) => JSON.stringify(e && e.data !== undefined ? e.data : ""))
  .join(" ");
const terminal = events.find((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR");
const streamErrors = events
  .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error")
  .map((e) => {
    if (e.data && e.data.value && typeof e.data.value === "object" && !Array.isArray(e.data.value)) {
      return e.data.value;
    }
    return e.data && typeof e.data === "object" ? e.data : { unknown: true };
  });
process.stdout.write(JSON.stringify({
  runId: runId,
  status: run ? run.status : "(none)",
  runtime: run ? (run.runtime ?? null) : null,
  provider: run ? (run.provider ?? null) : null,
  terminalKind: terminal ? terminal.type : null,
  terminalData: terminal ? terminal.data : null,
  assistantText: text,
  toolResultText,
  eventKinds: events.map((e) => e.type),
  streamErrors,
  outputCount: Array.isArray(outputs) ? outputs.length : 0
}));
`;

/**
 * Assemble a full runner script. `setup` (optional) runs first and may
 * `await` (e.g. AgentsMd.fromContent); `submit` is the object literal /
 * expression passed to `client.submit(...)` and must assign nothing —
 * the helper wraps it as `const runId = await client.submit(<submit>);`.
 */
export function sdkRunnerScript(parts: { readonly setup?: string; readonly submit: string }): string {
  return `${PREAMBLE}\n${parts.setup ?? ""}\nconst runId = await client.submit(${parts.submit});\n${TAIL}`;
}

/** Write + run a runner script in the install dir; parse the result JSON. */
export async function runSdkScript(
  install: InstallResult,
  env: UserEnv,
  script: string,
  opts: { readonly scriptName: string; readonly waitMs?: number; readonly timeoutMs?: number } = { scriptName: "sdk-run.mjs" }
): Promise<SdkRunResult> {
  const scriptPath = join(install.installDir, opts.scriptName);
  writeFileSync(scriptPath, script);

  const passEnv: Record<string, string> = {
    AEX_API_URL: env.apiBase,
    AEX_API_TOKEN: env.apiToken,
    MODEL_DEEPSEEK: env.deepseekModel,
    WAIT_MS: String(opts.waitMs ?? 240_000),
    ...(env.deepseekKey ? { DEEPSEEK_KEY: env.deepseekKey } : {})
  };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) passEnv[pathKey] = process.env[pathKey]!;
  const carry =
    process.platform === "win32"
      ? ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]
      : ["HOME", "TMPDIR", "LANG", "LC_ALL"];
  for (const k of carry) if (process.env[k]) passEnv[k] = process.env[k]!;

  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs: opts.timeoutMs ?? 5 * 60 * 1000,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `SDK runner exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as SdkRunResult;
}
