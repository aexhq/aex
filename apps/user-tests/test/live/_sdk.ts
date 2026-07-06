/**
 * Shared scaffolding for the config-fix USER tests (SDK-driven, customer
 * perspective). These differ from raw API probes; THESE drive the
 * installed `aex` SDK end-to-end
 * (SDK → /runs → runtime → events), the real customer surface.
 *
 * Each test installs the SDK (install fixture), then runs a small Bun script
 * IN the install dir that builds a submission via the SDK's classes
 * (Aex/AgentsMd/…), submits, polls to terminal, and
 * prints a standard result JSON which the test asserts on.
 *
 * They validate the FIXED behaviour and so only pass once the fixes are
 * DEPLOYED to the remote hosted API. Env mirrors the other user-tests:
 *   AEX_API_URL, AEX_API_KEY,
 *   DEEPSEEK_API_KEY,
 *   AEX_USER_TEST_TARBALL | AEX_USER_TEST_VERSION (the SDK to install).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { getBunCommand, runCommand, type InstallResult } from "../_fixtures/install.js";

export interface UserEnv {
  readonly apiBase: string;
  readonly apiKey: string;
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
    apiKey: req("AEX_API_KEY"),
    deepseekModel: process.env.AEX_USER_TEST_DEEPSEEK_MODEL?.trim() || "deepseek-v4-flash"
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
 * in-script: `client`, `DEEPSEEK_KEY`, `MODEL_DEEPSEEK`, and `AgentsMd`.
 */
const PREAMBLE = `
import { Aex, AgentsMd } from "@aexhq/sdk";
const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
const DEEPSEEK_KEY = process.env.DEEPSEEK_KEY;
const MODEL_DEEPSEEK = process.env.MODEL_DEEPSEEK;
`;

/**
 * Read the settle-consistent RunResult that `client.run(...)` returns
 * (events/outputs/text are already collected — no poll loop) + print the
 * standard result JSON.
 */
const TAIL = `
const fallbackEvents = Array.isArray(result.events) ? result.events : [];
let events = fallbackEvents;
let listedSession = null;
try {
  listedSession = typeof result.runId === "string" && result.runId
    ? await client.sessions.open(result.runId)
    : null;
  if (listedSession) {
    const listedEvents = await listedSession.events().list();
    if (Array.isArray(listedEvents) && listedEvents.length > 0) {
      events = listedEvents;
    }
  }
} catch {
  events = fallbackEvents;
}
const fallbackOutputs = Array.isArray(result.outputs) ? result.outputs : [];
let outputs = fallbackOutputs;
if (listedSession) {
  try {
    const listedOutputs = await listedSession.outputs().list();
    if (Array.isArray(listedOutputs)) {
      outputs = listedOutputs;
    }
  } catch {
    outputs = fallbackOutputs;
  }
}
const text = typeof result.text === "string" ? result.text : "";
const toolResultText = events
  .filter((e) => e.type === "TOOL_CALL_RESULT")
  .map((e) => JSON.stringify(e && e.data !== undefined ? e.data : ""))
  .join(" ");
const SESSION_TERMINAL_NAMES = new Set([
  "aex.session.idle",
  "aex.session.suspended",
  "aex.session.succeeded",
  "aex.session.failed",
  "aex.session.timed_out",
  "aex.session.cancelled"
]);
function isSessionIdle(e) {
  return e && e.type === "CUSTOM" && e.data && SESSION_TERMINAL_NAMES.has(e.data.name);
}
function terminalKindOf(e) {
  if (!e) return null;
  return isSessionIdle(e) ? "RUN_FINISHED" : e.type;
}
function terminalDataOf(e) {
  if (!e) return null;
  if (isSessionIdle(e)) {
    const value = e.data && e.data.value && typeof e.data.value === "object" ? e.data.value : {};
    return { ...value, reason: value.reason === "completed" ? "complete" : value.reason };
  }
  return e.data;
}
const terminal = events.find((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR") ?? events.find(isSessionIdle);
const eventKinds = events.map((e) => e.type);
if (terminal && isSessionIdle(terminal) && !eventKinds.includes("RUN_FINISHED")) {
  eventKinds.push("RUN_FINISHED");
}
const streamErrors = events
  .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error")
  .map((e) => {
    if (e.data && e.data.value && typeof e.data.value === "object" && !Array.isArray(e.data.value)) {
      return e.data.value;
    }
    return e.data && typeof e.data === "object" ? e.data : { unknown: true };
  });
// A one-shot run() parks the session cleanly on success (idle/suspended);
// surface that as "succeeded" so callers keep the run-oriented contract.
const status = result.ok
  ? "succeeded"
  : (typeof result.status === "string" && result.status ? result.status : "failed");
process.stdout.write(JSON.stringify({
  runId: result.runId,
  status,
  runtime: "managed",
  provider: (result.run && typeof result.run.provider === "string") ? result.run.provider : null,
  terminalKind: terminalKindOf(terminal),
  terminalData: terminalDataOf(terminal),
  assistantText: text,
  toolResultText,
  eventKinds,
  streamErrors,
  outputCount: outputs.length
}));
`;

/**
 * Assemble a full runner script. `setup` (optional) runs first and may
 * `await` (e.g. AgentsMd.fromContent); `run` is the object literal /
 * expression passed to `client.run(...)` and must assign nothing — the helper
 * wraps it as `const result = await client.run(<run>, { timeoutMs });`. The
 * `run` object is the session/run surface: `message` (the first turn), `apiKeys`
 * (BYOK provider keys), plus the usual composition inputs.
 */
export function sdkRunnerScript(parts: { readonly setup?: string; readonly run: string }): string {
  return `${PREAMBLE}\n${parts.setup ?? ""}\nconst result = await client.run(${parts.run}, { timeoutMs: Number(process.env.WAIT_MS || "240000") });\n${TAIL}`;
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
    AEX_API_KEY: env.apiKey,
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
