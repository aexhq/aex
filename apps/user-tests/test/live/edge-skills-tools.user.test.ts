/**
 * Live edge-case sweep for the SKILLS & TOOLS composition surface, from a real
 * customer's seat, against the DEV plane via the installed `@aexhq/sdk` on the
 * managed runtime (gate provider DeepSeek, BYOK `apiKeys`, model
 * `deepseek-v4-flash`, tiny prompts). Each case drives one live run and reduces
 * the event stream to observable assertions.
 *
 * Cases (5 live runs):
 *   1. A custom Tool that THROWS — the error must surface cleanly to the model
 *      as an `isError` tool result and the run must still finish (a tool throw
 *      is recoverable, not a run-killer).
 *   2. Builtin subset selection — `includeBuiltinTools:false` + a single
 *      cherry-picked `BuiltinTools.bash` gives the model exactly that one
 *      builtin, and it uses it (marker echoed).
 *   3. Custom Tool + full builtins — `includeBuiltinTools:true` alongside a
 *      custom Tool: the custom tool is invoked and returns its deterministic
 *      value while builtins coexist.
 *   4. Duplicate custom-tool NAME — two distinct Tools named `dup_tool` (backed
 *      by the offline wire test proving BOTH ride the wire). The collision must
 *      be handled cleanly server-side: either the run completes (dedup/shadow)
 *      or it fails with a structured error — never a silent hang / SDK crash.
 *   5. Empty tools + `includeBuiltinTools:false` — a run with zero tools still
 *      completes cleanly and the model answers from memory (marker echoed).
 *
 * Gating mirrors the sibling live suites: `requireEnv` throws at module load
 * when a credential is missing, so the file is only collected/run with live
 * creds. Required env:
 *   AEX_API_URL                live hosted API URL
 *   AEX_API_KEY              workspace API key
 *   DEEPSEEK_API_KEY          customer gate-provider (DeepSeek) key
 *   AEX_USER_TEST_TARBALL      packed SDK tarball  (OR AEX_USER_TEST_VERSION)
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-skills-tools): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const providerKey = requireGateKey("edge-skills-tools");
const model = gateModel();

const RUN_TIMEOUT_MS = 5 * 60_000;
const CHILD_TIMEOUT_MS = 7 * 60_000;
const IT_TIMEOUT_MS = 8 * 60_000;

interface ObservedToolCall {
  readonly id: string | null;
  readonly name: string;
}
interface ObservedToolResult {
  readonly id: string | null;
  readonly name: string | null;
  readonly isError: boolean;
  readonly text: string;
}
interface Observation {
  readonly threw: string | null;
  readonly runId: string | null;
  readonly status: string;
  readonly errorMessage: string | null;
  readonly terminalKind: string | null;
  readonly terminalReason: unknown;
  readonly eventKinds: readonly string[];
  readonly toolCalls: readonly ObservedToolCall[];
  readonly toolResults: readonly ObservedToolResult[];
  readonly skillLoadedNames: readonly string[];
  readonly assistantText: string;
  readonly assistantTextEventCount: number;
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
  readonly leakedProviderKey: boolean;
}

function buildPassEnv(extras: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extras };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  if (process.platform === "win32") {
    for (const k of ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  } else {
    for (const k of ["HOME", "TMPDIR", "LANG", "LC_ALL"]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  }
  return env;
}

// Child preamble: gate-provider managed client + a failure-tolerant observe().
const SCRIPT_PREAMBLE = `
import { Aex, BuiltinTools, Tool } from "@aexhq/sdk";

const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
const MODEL = process.env.MODEL;
const PROVIDER = process.env.PROVIDER;
const PROVIDER_KEY = process.env.PROVIDER_KEY;

function eventData(e) { return e && e.data && typeof e.data === "object" ? e.data : {}; }
function customName(e) { const d = eventData(e); return typeof d.name === "string" ? d.name : null; }
function customValue(e) { const d = eventData(e); const v = d.value; return v && typeof v === "object" ? v : {}; }
const SESSION_TERMINAL_NAMES = new Set(["aex.session.idle", "aex.session.suspended", "aex.session.succeeded", "aex.session.failed", "aex.session.timed_out", "aex.session.cancelled"]);
function isSessionIdle(e) { return e && e.type === "CUSTOM" && e.data && SESSION_TERMINAL_NAMES.has(e.data.name); }
function terminalKindOf(e) { if (!e) return null; return isSessionIdle(e) ? "RUN_FINISHED" : e.type; }
function terminalReasonOf(e) {
  if (!e) return null;
  if (isSessionIdle(e)) { const v = customValue(e); return v.reason === "completed" ? "complete" : (v.reason ?? null); }
  return eventData(e).reason ?? null;
}
function skillLoadedName(e) {
  const v = customValue(e);
  if (customName(e) === "aex.skill_loaded" || v.kind === "skill_loaded" || v.kind === "skill_loaded_marker") {
    const n = v.name || v.skillId; return typeof n === "string" ? n : null;
  }
  return null;
}
function blockText(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.map((b) => (typeof b === "string" ? b : b && typeof b.text === "string" ? b.text : "")).join("\\n");
}

async function observe(result, threw) {
  if (!result) {
    return {
      threw: threw ?? "unknown", runId: null, status: "threw", errorMessage: threw ?? null,
      terminalKind: null, terminalReason: null, eventKinds: [], toolCalls: [], toolResults: [],
      skillLoadedNames: [], assistantText: "", assistantTextEventCount: 0, streamErrors: [], leakedProviderKey: false
    };
  }
  const runId = typeof result.runId === "string" ? result.runId : null;
  let events = Array.isArray(result.events) ? result.events : [];
  try {
    if (runId) {
      const session = await client.sessions.open(runId);
      const listed = await session.events().list();
      if (Array.isArray(listed) && listed.length > 0) events = listed;
    }
  } catch { /* keep fallback events */ }

  const nameById = new Map();
  const toolCalls = events.filter((e) => e.type === "TOOL_CALL_START").map((e) => {
    const d = eventData(e);
    const id = typeof d.id === "string" ? d.id : null;
    const name = typeof d.name === "string" ? d.name : "";
    if (id && name) nameById.set(id, name);
    return { id, name };
  });
  const toolResults = events.filter((e) => e.type === "TOOL_CALL_RESULT").map((e) => {
    const d = eventData(e);
    const id = typeof d.id === "string" ? d.id : null;
    return { id, name: id ? (nameById.get(id) ?? null) : null, isError: d.isError === true, text: blockText(d.content).slice(0, 1500) };
  });
  const customEvents = events.filter((e) => e.type === "CUSTOM");
  const skillLoadedNames = customEvents.map(skillLoadedName).filter(Boolean);
  const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
  const assistantText = assistantTextEvents.map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : "")).join(" ");
  const terminal = events.find((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR") ?? events.find(isSessionIdle);
  const eventKinds = events.map((e) => e.type);
  if (terminal && isSessionIdle(terminal) && !eventKinds.includes("RUN_FINISHED")) eventKinds.push("RUN_FINISHED");
  const streamErrors = customEvents.filter((e) => customName(e) === "aex.stream_error").map((e) => customValue(e));
  const status = result.ok ? "succeeded" : (typeof result.status === "string" && result.status ? result.status : "failed");
  const errorMessage =
    (result.session && typeof result.session.errorMessage === "string" && result.session.errorMessage) ? result.session.errorMessage : null;
  const serialized = JSON.stringify({ events, status, errorMessage });
  return {
    threw: null, runId, status, errorMessage,
    terminalKind: terminalKindOf(terminal), terminalReason: terminalReasonOf(terminal),
    eventKinds, toolCalls, toolResults, skillLoadedNames,
    assistantText: assistantText.slice(0, 2000), assistantTextEventCount: assistantTextEvents.length,
    streamErrors, leakedProviderKey: PROVIDER_KEY.length > 0 && serialized.includes(PROVIDER_KEY)
  };
}

async function runOne(runArgs) {
  let result = null, threw = null;
  try {
    result = await client.run(runArgs, { timeoutMs: ${RUN_TIMEOUT_MS} });
  } catch (e) {
    threw = e && e.message ? e.message : String(e);
  }
  process.stdout.write(JSON.stringify(await observe(result, threw)));
  process.exit(0);
}
`;

async function runScenario(install: InstallResult, scriptName: string, body: string): Promise<{ observation: Observation; stdout: string }> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, `${SCRIPT_PREAMBLE}\n${body}\n`);
  const passEnv = buildPassEnv({ AEX_API_URL: apiUrl, AEX_API_KEY: apiKey, PROVIDER: GATE_PROVIDER, PROVIDER_KEY: providerKey, MODEL: model });
  const child = await runCommand(getBunCommand(), [scriptPath], { cwd: install.installDir, timeoutMs: CHILD_TIMEOUT_MS, env: passEnv });
  if (child.exitCode !== 0) {
    throw new Error(`${scriptName} exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
  }
  return { observation: JSON.parse(child.stdout.trim()) as Observation, stdout: child.stdout };
}

function tag(): string {
  return Math.random().toString(36).slice(2, 10).toUpperCase();
}
function toolCallNames(o: Observation): readonly string[] {
  return o.toolCalls.map((c) => c.name).filter(Boolean);
}
function norm(s: string): string {
  return s.replace(/\s+/g, "");
}
function diag(o: Observation): string {
  return [
    `threw=${o.threw}`,
    `runId=${o.runId} status=${o.status} errorMessage=${JSON.stringify(o.errorMessage)}`,
    `terminalKind=${o.terminalKind} terminalReason=${JSON.stringify(o.terminalReason)}`,
    `eventKinds=[${o.eventKinds.join(", ")}]`,
    `toolCalls=[${toolCallNames(o).join(", ")}]`,
    `toolResults=${JSON.stringify(o.toolResults).slice(0, 900)}`,
    o.streamErrors.length > 0 ? `streamErrors=${JSON.stringify(o.streamErrors).slice(0, 700)}` : "streamErrors=[]",
    `assistantText=${o.assistantText.slice(0, 500)}`
  ].join("\n");
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("live edge: skills & tools composition (gate provider, managed)", () => {
  it(
    "1. a throwing custom Tool surfaces an isError result and the run still finishes",
    async () => {
      const marker = "BOOM-" + tag();
      const indexSrc = `export default async function ({ input }) { throw new Error(${JSON.stringify("boom-thrown " + marker)}); }`;
      const body = `
await runOne({
  provider: PROVIDER,
  model: MODEL,
  system: "Call the boom_tool tool exactly once with x set to \\"go\\". It will return an error. After that, reply in one short sentence that the tool errored, and stop. Do not retry the tool.",
  message: "Call the boom_tool tool once with x=\\"go\\".",
  includeBuiltinTools: false,
  tools: [await Tool.fromFiles({
    name: "boom_tool",
    description: "Always throws an error when called.",
    inputSchema: { type: "object", properties: { x: { type: "string" } }, required: ["x"], additionalProperties: false },
    entry: "index.mjs",
    files: { "index.mjs": ${JSON.stringify(indexSrc)} }
  })],
  apiKeys: { [PROVIDER]: PROVIDER_KEY },
  idempotencyKey: "edge-throw-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-throw.mjs", body);
      const dump = diag(observation);

      // A throwing tool must NOT reject the SDK call nor kill the run.
      expect(observation.threw, dump).toBeNull();
      expect(observation.status, dump).toBe("succeeded");
      expect(observation.terminalKind, dump).toBe("RUN_FINISHED");
      expect(observation.terminalReason, dump).toBe("complete");

      // The model actually invoked the tool.
      expect(toolCallNames(observation), dump).toContain("boom_tool");

      // The throw surfaced as a tool result cleanly: a boom_tool result exists and
      // it is flagged isError (or at minimum carries error text) — not a crash.
      const r = observation.toolResults.find((x) => x.name === "boom_tool") ?? observation.toolResults[0];
      expect(r, dump).toBeDefined();
      const surfacedCleanly = r!.isError === true || /error|boom/i.test(r!.text);
      expect(surfacedCleanly, dump).toBe(true);

      expect(observation.assistantTextEventCount, dump).toBeGreaterThan(0);
      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(providerKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "2. includeBuiltinTools:false + a single cherry-picked builtin (bash) is available and used",
    async () => {
      const marker = "BLTN-" + tag();
      const body = `
await runOne({
  provider: PROVIDER,
  model: MODEL,
  system: "You have exactly one tool: bash. Use it to run the requested command, then reply with the exact printed line.",
  message: "Using your bash tool, run: printf '${marker}\\\\n'  — then reply with the exact line you printed and nothing else.",
  includeBuiltinTools: false,
  tools: [BuiltinTools.bash],
  apiKeys: { [PROVIDER]: PROVIDER_KEY },
  idempotencyKey: "edge-cherry-bash-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-cherry-bash.mjs", body);
      const dump = diag(observation);

      expect(observation.threw, dump).toBeNull();
      expect(observation.status, dump).toBe("succeeded");
      expect(observation.terminalKind, dump).toBe("RUN_FINISHED");
      expect(observation.terminalReason, dump).toBe("complete");

      // The single cherry-picked builtin was genuinely available and invoked.
      const bashCalls = toolCallNames(observation).filter((n) => n === "bash" || n === "shell");
      expect(bashCalls.length, dump).toBeGreaterThan(0);
      // And the printed marker made it back into the reply.
      expect(norm(observation.assistantText), dump).toContain(marker);

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(providerKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "3. includeBuiltinTools:true + a custom Tool coexist; the custom tool is invoked",
    async () => {
      const marker = "ECHO-" + tag();
      const indexSrc = `export default async function ({ input }) { return ${JSON.stringify("stamp:")} + String(input.v); }`;
      const body = `
await runOne({
  provider: PROVIDER,
  model: MODEL,
  system: "Use the echo_stamp tool for this task; do not answer from memory. After calling it, reply with its exact result on one line.",
  message: "Call the echo_stamp tool with v set to \\"${marker}\\", then reply with its exact result verbatim.",
  includeBuiltinTools: true,
  tools: [await Tool.fromFiles({
    name: "echo_stamp",
    description: "Stamp a caller-provided value and echo it back as stamp:<value>.",
    inputSchema: { type: "object", properties: { v: { type: "string" } }, required: ["v"], additionalProperties: false },
    entry: "index.mjs",
    files: { "index.mjs": ${JSON.stringify(indexSrc)} }
  })],
  apiKeys: { [PROVIDER]: PROVIDER_KEY },
  idempotencyKey: "edge-custom-plus-builtins-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-custom-plus-builtins.mjs", body);
      const dump = diag(observation);

      expect(observation.threw, dump).toBeNull();
      expect(observation.status, dump).toBe("succeeded");
      expect(observation.terminalKind, dump).toBe("RUN_FINISHED");
      expect(observation.terminalReason, dump).toBe("complete");

      // The custom tool was invoked and executed cleanly, echoing stamp:<marker>.
      expect(toolCallNames(observation), dump).toContain("echo_stamp");
      const r = observation.toolResults.find((x) => x.name === "echo_stamp");
      expect(r, dump).toBeDefined();
      expect(r!.isError, dump).toBe(false);
      expect(norm(r!.text), dump).toContain(norm(`stamp:${marker}`));

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(providerKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "4. duplicate custom-tool NAME is handled cleanly (no silent hang / SDK crash)",
    async () => {
      const marker = tag();
      const srcA = `export default async function () { return ${JSON.stringify("dup-AAA-" + marker)}; }`;
      const srcB = `export default async function () { return ${JSON.stringify("dup-BBB-" + marker)}; }`;
      const body = `
await runOne({
  provider: PROVIDER,
  model: MODEL,
  system: "Call the dup_tool tool once with no arguments, then reply with its exact result.",
  message: "Call the dup_tool tool once (no arguments) and reply with its exact result.",
  includeBuiltinTools: false,
  tools: [
    await Tool.fromFiles({ name: "dup_tool", description: "Alpha variant.", inputSchema: { type: "object", properties: {} }, entry: "index.mjs", files: { "index.mjs": ${JSON.stringify(srcA)} } }),
    await Tool.fromFiles({ name: "dup_tool", description: "Bravo variant.", inputSchema: { type: "object", properties: {} }, entry: "index.mjs", files: { "index.mjs": ${JSON.stringify(srcB)} } })
  ],
  apiKeys: { [PROVIDER]: PROVIDER_KEY },
  idempotencyKey: "edge-dup-name-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-dup-name.mjs", body);
      const dump = diag(observation);

      // The submission with a duplicate tool name must be handled cleanly.
      // Acceptable: the run completes (server deduped / shadowed one), OR the
      // submission fails with a STRUCTURED error (SDK threw a message, or the run
      // ended failed with a diagnostic). Unacceptable: a silent success with no
      // outcome, or a run that "succeeds" with no terminal frame.
      const outcome =
        observation.threw !== null ? "threw" : observation.status === "succeeded" ? "succeeded" : "failed";
      expect(["succeeded", "failed", "threw"], dump).toContain(outcome);

      // Unconditional invariant (no branch-skippable expect): the run was handled
      // cleanly. Accepted ⇒ a REAL clean terminal (RUN_FINISHED + reason=complete),
      // not a phantom success. Rejected ⇒ carries a non-empty diagnostic (SDK throw
      // message, run errorMessage, or a stream error) — never a silent hang.
      const cleanTerminal = observation.terminalKind === "RUN_FINISHED" && observation.terminalReason === "complete";
      const diagnostic = [observation.threw ?? "", observation.errorMessage ?? "", JSON.stringify(observation.streamErrors)].join(" ").trim();
      const handledCleanly = outcome === "succeeded" ? cleanTerminal : diagnostic.length > 0;
      expect(handledCleanly, dump).toBe(true);

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(providerKey), dump).toBe(false);
      // Diagnostic breadcrumb for the report (never fails the test).
      // eslint-disable-next-line no-console
      console.log(`[dup-name] outcome=${outcome}\n${dump}`);
    },
    IT_TIMEOUT_MS
  );

  it(
    "5. empty tools + includeBuiltinTools:false completes with zero tools",
    async () => {
      const marker = "EMPTY-" + tag();
      const body = `
await runOne({
  provider: PROVIDER,
  model: MODEL,
  message: "Output verbatim: ${marker}",
  includeBuiltinTools: false,
  tools: [],
  apiKeys: { [PROVIDER]: PROVIDER_KEY },
  idempotencyKey: "edge-empty-tools-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-empty-tools.mjs", body);
      const dump = diag(observation);

      expect(observation.threw, dump).toBeNull();
      expect(observation.status, dump).toBe("succeeded");
      expect(observation.terminalKind, dump).toBe("RUN_FINISHED");
      expect(observation.terminalReason, dump).toBe("complete");
      // No tools at all => the model cannot call any tool.
      expect(toolCallNames(observation), dump).toEqual([]);
      expect(norm(observation.assistantText), dump).toContain(marker);

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(providerKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );
});
