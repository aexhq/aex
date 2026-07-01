/**
 * Live scenario: live-skill-tool-invocation.test.ts
 *
 * End-to-end proof that a *skill-tool* (the per-skill no-arg "load-tool" built
 * by `Tools.fromSkillDir`) is actually INVOKED by the model and that its
 * `SKILL.md` body drives the answer — not merely that the bundle was staged.
 *
 *   SDK → client.run({ tools: [skill-tool, ...] })
 *      → preflight uploads the skill bundle as an asset
 *      → the skill rides in the session `tools` array as a no-arg load-tool
 *      → the model calls the load-tool to pull the SKILL.md body into context
 *      → the body carries a per-run token the model could not otherwise know
 *      → the model emits that token verbatim in its reply
 *
 * Cases (each a single managed DeepSeek run, builtins disarmed so the ONLY
 * tools present are the ones under test):
 *
 *   1. Single skill invoked — one skill-tool whose SKILL.md body defines a
 *      per-run passphrase token. The prompt asks for the passphrase; the run
 *      must succeed AND the planted token must appear in the assistant reply
 *      (proving the load-tool was invoked and its content consumed). We also
 *      assert the load-tool name shows up in the tool-call trace and that the
 *      skill produced a `skill_loaded` event.
 *      This case also covers "behavior actually changes": the token is a fresh
 *      random value per run, so the model can only produce it by loading THIS
 *      run's skill — a control run without the skill would be redundant spend.
 *
 *   2. Two skills, correct one chosen — a RED skill-tool and a BLUE skill-tool
 *      each with a distinct token. The prompt asks for the RED passphrase only.
 *      The RED token must appear in the reply and the BLUE token must NOT
 *      (catches "all skills collapsed into one" / wrong-skill selection).
 *
 *   3. Skill-tool + custom Tool together — one run wiring a skill-tool AND a
 *      custom `Tool.fromFiles` tool. The prompt drives BOTH: the custom tool
 *      stamps a marker (deterministic echo) and the skill supplies the vault
 *      passphrase. Both must be invoked and produce their deterministic values.
 *
 * Assertions anchor on the deterministic PLANTED tokens (never on free-form
 * phrasing) so they are robust to LLM nondeterminism.
 *
 * Gating mirrors the sibling live suites exactly: `requireEnv` throws at module
 * load when a credential is missing, so the file is only collected/run with
 * live creds (the offline config excludes `test/live/**`). Required env:
 *   AEX_API_URL              live hosted API URL
 *   AEX_API_TOKEN            workspace API token
 *   DEEPSEEK_API_KEY         customer DeepSeek key
 *   AEX_USER_TEST_TARBALL          packed SDK tarball
 *     OR AEX_USER_TEST_VERSION     published package version
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (skill-tool-invocation): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

const RUN_TIMEOUT_MS = 6 * 60_000;
const CHILD_TIMEOUT_MS = 8 * 60_000;
const IT_TIMEOUT_MS = 10 * 60_000;

interface ObservedToolCall {
  readonly id: string | null;
  readonly name: string;
  readonly arguments: Readonly<Record<string, unknown>>;
}

interface ObservedToolResult {
  readonly id: string | null;
  readonly name: string | null;
  readonly isError: boolean;
  readonly text: string;
}

interface Observation {
  readonly runId: string;
  readonly status: string;
  readonly eventKinds: readonly string[];
  readonly terminalKind: string | null;
  readonly terminalData: Readonly<Record<string, unknown>> | null;
  readonly assistantText: string;
  readonly assistantTextEventCount: number;
  readonly toolCalls: readonly ObservedToolCall[];
  readonly toolResults: readonly ObservedToolResult[];
  readonly skillLoadedNames: readonly string[];
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
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

// Shared child-runner preamble. The child imports the freshly-installed SDK
// from the install tempdir (so workspace symlinks cannot leak), runs one live
// managed DeepSeek run, then reduces the raw event stream to a JSON
// `Observation` on stdout. Raw-event iteration mirrors the proven sibling
// harnesses (live-sdk-skill-invocation / live-sdk-tool-capability-fuzz):
//   - TOOL_CALL_START names → which tools (incl. skill load-tools) were invoked
//   - TOOL_CALL_RESULT content → deterministic tool outputs
//   - CUSTOM aex.skill_loaded → which skills were staged into the container
//   - TEXT_MESSAGE_CONTENT + result.text → the model's answer
const SCRIPT_PREAMBLE = `
import { AgentExecutor, Tools, Tool } from "@aexhq/sdk";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const client = new AgentExecutor({
  baseUrl: process.env.AEX_API_URL,
  apiToken: process.env.AEX_API_TOKEN
});
const MODEL = process.env.MODEL_DEEPSEEK;
const DEEPSEEK_KEY = process.env.DEEPSEEK_KEY;

// Skills are ingested as TOOLS: write the SKILL.md (YAML frontmatter carries
// the tool name + description) to a temp dir, then build a skill-tool from it.
function skillDir(md) {
  const dir = mkdtempSync(join(tmpdir(), "aex-skill-"));
  writeFileSync(join(dir, "SKILL.md"), md);
  return dir;
}

function eventData(e) {
  return e && e.data && typeof e.data === "object" ? e.data : {};
}
function customName(e) {
  const d = eventData(e);
  return typeof d.name === "string" ? d.name : null;
}
function customValue(e) {
  const d = eventData(e);
  const v = d.value;
  return v && typeof v === "object" ? v : {};
}
function isSessionIdle(e) {
  return e && e.type === "CUSTOM" && e.data && e.data.name === "aex.session.idle";
}
function terminalKindOf(e) {
  if (!e) return null;
  return isSessionIdle(e) ? "RUN_FINISHED" : e.type;
}
function terminalDataOf(e) {
  if (!e) return null;
  if (isSessionIdle(e)) {
    const v = customValue(e);
    return { ...v, reason: v.reason === "completed" ? "complete" : v.reason };
  }
  return eventData(e);
}
function skillLoadedName(e) {
  const v = customValue(e);
  if (
    customName(e) === "aex.skill_loaded" ||
    v.kind === "skill_loaded" ||
    v.kind === "skill_loaded_marker"
  ) {
    const name = v.name || v.skillId;
    return typeof name === "string" ? name : null;
  }
  return null;
}
function blockText(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((b) => (typeof b === "string" ? b : b && typeof b.text === "string" ? b.text : ""))
    .join("\\n");
}

async function observe(result) {
  const fallbackEvents = Array.isArray(result.events) ? result.events : [];
  let events = fallbackEvents;
  try {
    const session = await client.sessions.open(result.runId);
    const listedEvents = await session.events().list();
    if (Array.isArray(listedEvents) && listedEvents.length > 0) events = listedEvents;
  } catch {
    events = fallbackEvents;
  }
  const nameById = new Map();
  const toolCalls = events
    .filter((e) => e.type === "TOOL_CALL_START")
    .map((e) => {
      const d = eventData(e);
      const id = typeof d.id === "string" ? d.id : null;
      const name = typeof d.name === "string" ? d.name : "";
      if (id && name) nameById.set(id, name);
      const args =
        d.arguments && typeof d.arguments === "object" && !Array.isArray(d.arguments) ? d.arguments : {};
      return { id, name, arguments: args };
    });
  const toolResults = events
    .filter((e) => e.type === "TOOL_CALL_RESULT")
    .map((e) => {
      const d = eventData(e);
      const id = typeof d.id === "string" ? d.id : null;
      return {
        id,
        name: id ? nameById.get(id) ?? null : null,
        isError: d.isError === true,
        text: blockText(d.content)
      };
    });
  const customEvents = events.filter((e) => e.type === "CUSTOM");
  const skillLoadedNames = customEvents.map(skillLoadedName).filter(Boolean);
  const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
  const assistantTextJoined = assistantTextEvents
    .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
    .join(" ");
  const finalText = typeof result.text === "string" && result.text ? result.text + " " : "";
  const terminal = events.find((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR") ?? events.find(isSessionIdle);
  const eventKinds = events.map((e) => e.type);
  if (terminal && isSessionIdle(terminal) && !eventKinds.includes("RUN_FINISHED")) {
    eventKinds.push("RUN_FINISHED");
  }
  const streamErrors = customEvents
    .filter((e) => customName(e) === "aex.stream_error")
    .map((e) => (customValue(e) ? customValue(e) : { unknown: true }));
  return {
    runId: result.runId,
    status: result.ok
      ? "succeeded"
      : typeof result.status === "string" && result.status
        ? result.status
        : "failed",
    eventKinds,
    terminalKind: terminalKindOf(terminal),
    terminalData: terminalDataOf(terminal),
    assistantText: finalText + assistantTextJoined,
    assistantTextEventCount: assistantTextEvents.length,
    toolCalls,
    toolResults,
    skillLoadedNames,
    streamErrors
  };
}
`;

function buildScript(body: string): string {
  return `${SCRIPT_PREAMBLE}\n${body}\n`;
}

async function runScenario(
  install: InstallResult,
  scriptName: string,
  body: string
): Promise<{ readonly observation: Observation; readonly stdout: string }> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, buildScript(body));
  const passEnv = buildPassEnv({
    AEX_API_URL: apiUrl,
    AEX_API_TOKEN: apiToken,
    DEEPSEEK_KEY: deepseekKey,
    MODEL_DEEPSEEK: deepseekModel
  });
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs: CHILD_TIMEOUT_MS,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `${scriptName} exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return { observation: JSON.parse(child.stdout.trim()) as Observation, stdout: child.stdout };
}

// --- outer-harness helpers ------------------------------------------------

/** 8 lowercase-alnum chars — safe inside a skill/tool name (SKILL_NAME_PATTERN). */
function nameSuffix(): string {
  return (Math.random().toString(36) + "000000").slice(2, 10);
}

/** Fresh per-run token. Hyphen-segmented and < 24 chars so the runtime's
 * high-entropy `[A-Za-z0-9+/=-]{24,}` stream redactor never eats it, and free
 * of the `token`/`key`/`secret` keywords its key<sep>value rule keys off. */
function plantedToken(prefix: string): string {
  return `${prefix}-${nameSuffix().toUpperCase()}`;
}

/** A SKILL.md whose body is the ONLY source of `replyLine`. */
function buildSkillMd(name: string, description: string, trigger: string, replyLine: string): string {
  return [
    "---",
    `name: ${name}`,
    `description: ${description}`,
    "---",
    `# ${name}`,
    "",
    `This mounted skill is the ONLY place that defines the answer below. ` +
      `When the user asks for ${trigger}, reply with EXACTLY this one line, ` +
      `copied verbatim, and nothing else:`,
    "",
    replyLine,
    "",
    `Do not explain. Do not translate, summarize, or alter it. Reply with that single line only.`
  ].join("\n");
}

function toolCallNames(o: Observation): readonly string[] {
  return o.toolCalls.map((c) => c.name).filter(Boolean);
}

function normalize(s: string): string {
  return s.replace(/\s+/g, "");
}

function diagnostics(o: Observation): string {
  return [
    `runId=${o.runId} status=${o.status}`,
    `terminalKind=${o.terminalKind} terminalData=${JSON.stringify(o.terminalData)}`,
    `eventKinds=[${o.eventKinds.join(", ")}]`,
    `skillLoadedNames=[${o.skillLoadedNames.join(", ")}]`,
    `toolCalls=[${toolCallNames(o).join(", ")}]`,
    `toolResults=${JSON.stringify(o.toolResults).slice(0, 1200)}`,
    o.streamErrors.length > 0 ? `streamErrors=${JSON.stringify(o.streamErrors).slice(0, 800)}` : "streamErrors=[]",
    `assistantText=${o.assistantText.slice(0, 800)}`
  ].join("\n");
}

/** Clean managed terminal + a real assistant reply, shared by every case. */
function assertCleanTerminal(o: Observation): void {
  const dump = diagnostics(o);
  expect(o.status, dump).toBe("succeeded");
  expect(o.terminalKind, dump).toBe("RUN_FINISHED");
  expect(o.terminalData?.["reason"], dump).toBe("complete");
  expect(o.assistantTextEventCount, dump).toBeGreaterThan(0);
}

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("live skill-tool invocation — model calls the load-tool and follows SKILL.md", () => {
  it(
    "single skill: load-tool is invoked and its planted token drives the reply",
    async () => {
      const skillName = `vault-skill-${nameSuffix()}`;
      const token = plantedToken("SKILL-OK-7F");
      const replyLine = `vault-pass ${token}`;
      const md = buildSkillMd(
        skillName,
        "Reveal the one-time vault passphrase when the user asks for it.",
        "the vault passphrase",
        replyLine
      );
      const prompt =
        `What is the vault passphrase? Use the mounted ${skillName} tool to look it up, ` +
        `then reply with its \`vault-pass\` line verbatim and nothing else.`;
      const system =
        `Rely on the mounted ${skillName} skill for the vault passphrase. ` +
        `Do not answer from general memory; call the tool and copy its reply line exactly.`;

      const body = `
const skill = await Tools.fromSkillDir(skillDir(${JSON.stringify(md)}), { name: ${JSON.stringify(skillName)} });
const result = await client.run({
  provider: "deepseek",
  model: MODEL,
  system: ${JSON.stringify(system)},
  message: ${JSON.stringify(prompt)},
  includeBuiltinTools: false,
  tools: [skill],
  apiKeys: { deepseek: DEEPSEEK_KEY },
  idempotencyKey: "skill-tool-single-" + Date.now()
}, { timeoutMs: ${RUN_TIMEOUT_MS} });
process.stdout.write(JSON.stringify(await observe(result)));
`;
      const { observation, stdout } = await runScenario(install, "skill-tool-single.mjs", body);
      const dump = diagnostics(observation);

      assertCleanTerminal(observation);

      // The skill bundle was staged into the container.
      expect(observation.skillLoadedNames, dump).toContain(skillName);

      // The model INVOKED the no-arg load-tool (its name rides the tool-call trace).
      expect(toolCallNames(observation), dump).toContain(skillName);

      // The per-run token — unknowable without loading THIS run's skill — is in
      // the answer, proving the SKILL.md body drove the reply (also covers the
      // "behavior actually changes" requirement without a costly control run).
      expect(normalize(observation.assistantText), dump).toContain(normalize(token));

      // No provider key leaks into the SDK-visible payload.
      expect(stdout.includes(deepseekKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "two skills: the prompt-selected skill wins and the distractor token never appears",
    async () => {
      const redName = `red-skill-${nameSuffix()}`;
      const blueName = `blue-skill-${nameSuffix()}`;
      const redToken = plantedToken("RED-7F");
      const blueToken = plantedToken("BLUE-7F");
      const redReply = `passphrase-red ${redToken}`;
      const blueReply = `passphrase-blue ${blueToken}`;
      const redMd = buildSkillMd(
        redName,
        "Reveal the RED passphrase when the user asks for the RED passphrase.",
        "the RED passphrase",
        redReply
      );
      const blueMd = buildSkillMd(
        blueName,
        "Reveal the BLUE passphrase when the user asks for the BLUE passphrase.",
        "the BLUE passphrase",
        blueReply
      );
      const prompt =
        `What is the RED passphrase? Use ONLY the mounted ${redName} tool. ` +
        `Do NOT use the ${blueName} tool. Reply with its \`passphrase-red\` line verbatim and nothing else.`;
      const system =
        `Two skills are mounted. The RED passphrase lives in ${redName}; the BLUE passphrase lives in ${blueName}. ` +
        `Answer a RED request using ONLY ${redName}. Never disclose the BLUE passphrase.`;

      const body = `
const red = await Tools.fromSkillDir(skillDir(${JSON.stringify(redMd)}), { name: ${JSON.stringify(redName)} });
const blue = await Tools.fromSkillDir(skillDir(${JSON.stringify(blueMd)}), { name: ${JSON.stringify(blueName)} });
const result = await client.run({
  provider: "deepseek",
  model: MODEL,
  system: ${JSON.stringify(system)},
  message: ${JSON.stringify(prompt)},
  includeBuiltinTools: false,
  tools: [red, blue],
  apiKeys: { deepseek: DEEPSEEK_KEY },
  idempotencyKey: "skill-tool-two-" + Date.now()
}, { timeoutMs: ${RUN_TIMEOUT_MS} });
process.stdout.write(JSON.stringify(await observe(result)));
`;
      const { observation, stdout } = await runScenario(install, "skill-tool-two.mjs", body);
      const dump = diagnostics(observation);

      assertCleanTerminal(observation);

      // Both skills staged (proves they did not collapse into one wire ref).
      expect(observation.skillLoadedNames, dump).toContain(redName);
      expect(observation.skillLoadedNames, dump).toContain(blueName);

      // The correct load-tool was invoked and its token drove the reply.
      expect(toolCallNames(observation), dump).toContain(redName);
      const normalizedAnswer = normalize(observation.assistantText);
      expect(normalizedAnswer, dump).toContain(normalize(redToken));

      // The distractor token must NOT surface — wrong-skill selection / skill
      // collapse would leak it.
      expect(normalizedAnswer.includes(normalize(blueToken)), dump).toBe(false);

      expect(stdout.includes(deepseekKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "skill-tool + custom Tool: both are invoked and produce their deterministic values",
    async () => {
      const skillName = `vault-skill-${nameSuffix()}`;
      const token = plantedToken("SKILL-OK-7F");
      const replyLine = `vault-pass ${token}`;
      const md = buildSkillMd(
        skillName,
        "Reveal the one-time vault passphrase when the user asks for it.",
        "the vault passphrase",
        replyLine
      );
      const stamp = plantedToken("STAMP");
      const prompt =
        `Do BOTH steps, in order. ` +
        `1) Call the stamp_marker tool with marker set to "${stamp}". ` +
        `2) Use the mounted ${skillName} tool to look up the vault passphrase. ` +
        `Then reply on one line containing the stamp_marker result and the skill's \`vault-pass\` line, verbatim.`;
      const system =
        `Use the stamp_marker tool to stamp the marker, and the mounted ${skillName} skill for the ` +
        `vault passphrase. Do not answer either from memory; call both tools.`;

      const body = `
const skill = await Tools.fromSkillDir(skillDir(${JSON.stringify(md)}), { name: ${JSON.stringify(skillName)} });
const stampTool = await Tool.fromFiles({
  name: "stamp_marker",
  description: "Stamp a caller-provided marker and echo it back verbatim.",
  inputSchema: {
    type: "object",
    properties: { marker: { type: "string" } },
    required: ["marker"],
    additionalProperties: false
  },
  entry: "index.mjs",
  files: {
    "index.mjs": "export default async function ({ input }) { return 'stamped:' + String(input.marker); }"
  }
});
const result = await client.run({
  provider: "deepseek",
  model: MODEL,
  system: ${JSON.stringify(system)},
  message: ${JSON.stringify(prompt)},
  includeBuiltinTools: false,
  tools: [skill, stampTool],
  apiKeys: { deepseek: DEEPSEEK_KEY },
  idempotencyKey: "skill-tool-plus-custom-" + Date.now()
}, { timeoutMs: ${RUN_TIMEOUT_MS} });
process.stdout.write(JSON.stringify(await observe(result)));
`;
      const { observation, stdout } = await runScenario(install, "skill-tool-plus-custom.mjs", body);
      const dump = diagnostics(observation);

      assertCleanTerminal(observation);

      // Skill staged + skill load-tool invoked + its planted token in the reply.
      expect(observation.skillLoadedNames, dump).toContain(skillName);
      expect(toolCallNames(observation), dump).toContain(skillName);
      expect(normalize(observation.assistantText), dump).toContain(normalize(token));

      // Custom Tool invoked and executed cleanly, echoing the exact marker.
      expect(toolCallNames(observation), dump).toContain("stamp_marker");
      const stampResult = observation.toolResults.find((r) => r.name === "stamp_marker");
      expect(stampResult, dump).toBeDefined();
      expect(stampResult?.isError, dump).toBe(false);
      expect(normalize(stampResult?.text ?? ""), dump).toContain(normalize(`stamped:${stamp}`));

      expect(stdout.includes(deepseekKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );
});
