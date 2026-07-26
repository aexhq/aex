/**
 * Shared scaffolding for the config-fix USER tests (SDK-driven, customer
 * perspective). These differ from raw API probes; THESE drive the
 * installed `aex` SDK end-to-end
 * (SDK → /api/sessions → runtime → events), the real customer surface.
 *
 * Each test installs the SDK (install fixture), then runs a small Bun script
 * IN the install dir that builds a submission via the SDK's classes
 * (Aex/Instructions/…), submits, waits for the RUN terminal, and
 * prints a standard result JSON which the test asserts on.
 *
 * They validate the FIXED behaviour and so only pass once the fixes are
 * DEPLOYED to the remote hosted API. Env mirrors the other user-tests:
 *   AEX_API_URL, AEX_API_KEY,
 *   AEX_USER_TEST_TARBALL | AEX_USER_TEST_VERSION (the SDK to install).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { getBunCommand, runCommand, type InstallResult } from "../_fixtures/install.js";

export interface UserEnv {
  readonly apiBase: string;
  readonly apiKey: string;
  readonly deepseekModel: string;
}

function req(name: string): string {
  const v = process.env[name];
  if (!v || v.length === 0) {
    throw new Error(
      `user-tests: required env ${name} is missing. The SDK config-fix tests run against a real hosted API.`
    );
  }
  return v;
}

export function requireUserEnv(_opts: { deepseek?: boolean } = {}): UserEnv {
  return {
    apiBase: req("AEX_API_URL").replace(/\/$/, ""),
    apiKey: req("AEX_API_KEY"),
    deepseekModel: process.env.AEX_USER_TEST_DEEPSEEK_MODEL?.trim() || "deepseek/deepseek-v4-flash"
  };
}

/** Whitespace-stripped text — streamed assistant events fragment tokens. */
export function dense(s: string): string {
  return s.replace(/\s+/g, "");
}

export function observedSessionText(result: SdkSessionResult): string {
  return dense([result.assistantText, result.toolResultText].join(" "));
}

export function runDiagnostics(result: SdkSessionResult): string {
  return [
    `sessionId=${result.sessionId}`,
    `status=${result.status}`,
    `runtime=${result.runtime ?? "(none)"}`,
    `provider=${result.provider ?? "(none)"}`,
    `terminalKind=${result.terminalKind ?? "(none)"}`,
    `events=[${result.eventKinds.join(", ")}]`,
    `assistantText=${JSON.stringify(result.assistantText).slice(0, 500)}`,
    `toolResultText=${JSON.stringify(result.toolResultText).slice(0, 500)}`,
    `streamErrors=${JSON.stringify(result.streamErrors).slice(0, 500)}`,
    `fileCount=${result.fileCount}`
  ].join("\n");
}

export interface SdkSessionResult {
  readonly sessionId: string;
  readonly status: string;
  readonly runtime: string | null;
  readonly provider: string | null;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly assistantText: string;
  readonly toolResultText: string;
  readonly eventKinds: readonly string[];
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
  readonly fileCount: number;
}

/**
 * Script preamble: imports the SDK + builds the client from env. Available
 * in-script: `client`, `MODEL_DEEPSEEK`, and `Instructions`.
 */
const PREAMBLE = `
import { Aex, Instructions } from "@aexhq/sdk";
const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
const MODEL_DEEPSEEK = process.env.MODEL_DEEPSEEK;
`;

/**
 * Read the checkpoint-consistent SessionResult that `client.start(...)` returns
 * (events/files/text are already collected — no poll loop) + print the
 * standard result JSON.
 */
const TAIL = `
if (!Array.isArray(result.events)) throw new Error("start() result.events must be an array");
if (!Array.isArray(result.files)) throw new Error("start() result.files must be an array");
const events = result.events;
const files = result.files;
const text = typeof result.text === "string" ? result.text : "";
const toolResultText = events
  .filter((e) => e.type === "TOOL_CALL_RESULT")
  .map((e) => JSON.stringify(e && e.data !== undefined ? e.data : ""))
  .join(" ");
const terminals = events.filter((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR");
if (terminals.length !== 1) throw new Error("expected exactly one RUN terminal, got " + terminals.length);
const terminal = terminals[0];
if (!terminal.data || typeof terminal.data !== "object") throw new Error("RUN terminal data is required");
if (terminal.data.outcome !== result.status) throw new Error("RUN terminal outcome must match result.status");
if (terminal.type === "RUN_ERROR" && terminal.data.outcome !== "failed") throw new Error("RUN_ERROR must carry outcome=failed");
if (terminal.type === "RUN_FINISHED" && terminal.data.outcome === "failed") throw new Error("failed runs must use RUN_ERROR");
if (typeof terminal.data.costUsd !== "number" || !Array.isArray(terminal.data.providerUsage)) {
  throw new Error("RUN terminal must carry per-run costUsd and providerUsage");
}
if (terminal.type === "RUN_FINISHED" && (!terminal.data.checkpoint || typeof terminal.data.checkpoint.checkpointId !== "string")) {
  throw new Error("RUN_FINISHED must carry its committed checkpoint");
}
const eventKinds = events.map((e) => e.type);
const streamErrors = events
  .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error")
  .map((e) => {
    if (e.data && e.data.value && typeof e.data.value === "object" && !Array.isArray(e.data.value)) {
      return e.data.value;
    }
    return e.data && typeof e.data === "object" ? e.data : { unknown: true };
  });
const status = typeof result.status === "string" && result.status ? result.status : "failed";
process.stdout.write(JSON.stringify({
  sessionId: result.sessionId,
  status,
  runtime: "managed",
  provider: (result.session && typeof result.session.provider === "string") ? result.session.provider : null,
  terminalKind: terminal.type,
  terminalData: terminal.data,
  assistantText: text,
  toolResultText,
  eventKinds,
  streamErrors,
  fileCount: files.length
}));
`;

/**
 * Assemble a full runner script. `setup` (optional) runs first and may
 * `await` (e.g. publishing `Instructions.fromContent`); `run` is the object literal /
 * expression passed to `client.start(...)` and must assign nothing — the helper
 * wraps it as `const result = await client.start(<run>, { timeoutMs });`. The
 * `run` object is the session/run surface: `message` (the first turn) plus the
 * usual composition inputs. Model access needs no key — the managed gateway serves it.
 */
export function sdkRunnerScript(parts: { readonly setup?: string; readonly session: string }): string {
  return `${PREAMBLE}\n${parts.setup ?? ""}\nconst result = await client.start(${parts.session}, { timeoutMs: Number(process.env.WAIT_MS || "240000") });\n${TAIL}`;
}

/** Write + run a runner script in the install dir; parse the result JSON. */
export async function runSdkScript(
  install: InstallResult,
  env: UserEnv,
  script: string,
  opts: { readonly scriptName: string; readonly waitMs?: number; readonly timeoutMs?: number } = { scriptName: "sdk-run.mjs" }
): Promise<SdkSessionResult> {
  const scriptPath = join(install.installDir, opts.scriptName);
  writeFileSync(scriptPath, script);

  const passEnv: Record<string, string> = {
    AEX_API_URL: env.apiBase,
    AEX_API_KEY: env.apiKey,
    MODEL_DEEPSEEK: env.deepseekModel,
    WAIT_MS: String(opts.waitMs ?? 240_000)
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
  return JSON.parse(child.stdout.trim()) as SdkSessionResult;
}
