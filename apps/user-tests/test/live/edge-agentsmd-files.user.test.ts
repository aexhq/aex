/**
 * Live edge-case sweep for the AGENTS.MD + FILES/ASSETS composition surface,
 * from a real customer's seat, against the DEV plane via the installed
 * `@aexhq/sdk` on the Anthropic managed runtime (BYOK `apiKeys: { anthropic }`,
 * model `claude-haiku-4-5`, tiny prompts). Each case drives one live run and
 * reduces the event stream / tool results to observable assertions.
 *
 * Cases (7 live runs):
 *   1. AgentsMd STEERS the run — `fromContent` instructs the model to prefix
 *      every reply with a random one-time token; the token must appear in the
 *      reply (proves the agents.md reached the agent AND changed its behaviour,
 *      not just that a benign fact was recalled).
 *   2. A File MATERIALIZES + the model reads it — `File.fromBytes` with a secret
 *      marker word, read back via `cat /workspace/notes.txt`; the marker must
 *      surface (file landed at its real filename under the workspace cwd).
 *   3. LARGE (~100 KB) agents.md still steers — a codename buried at the END of a
 *      100 KB handbook must be recalled (agents.md is not truncated).
 *   4. BINARY byte-exact round-trip — a File of bytes 0x00..0xff + a marker;
 *      `sha256sum` inside the runtime must equal the digest computed locally
 *      (raw bytes survived zip -> asset -> unzip intact).
 *   5. MULTIPLE files + agents.md COMPOSE — two files (each a secret word) plus
 *      an agents.md (a codename); a single answer needs all three.
 *   6. SECURITY: a File with `mountPath:"/etc"` must NOT escape the workspace —
 *      it must land rebased under /workspace, never at the real /etc.
 *   7. Tricky FILENAMES survive to disk — a file named "my report.txt" (space)
 *      and "café.txt" (unicode); both read back by their real on-disk names.
 *
 * Gating mirrors the sibling live suites: `requireEnv` throws at module load
 * when a credential is missing, so the file is only collected/run with live
 * creds. Required env:
 *   AEX_API_URL                live hosted API URL
 *   AEX_API_TOKEN              workspace API token
 *   ANTHROPIC_API_KEY          customer Anthropic key
 *   AEX_USER_TEST_TARBALL      packed SDK tarball  (OR AEX_USER_TEST_VERSION)
 */
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-agentsmd-files): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const anthropicKey = requireEnv("ANTHROPIC_API_KEY");
const model = process.env["AEX_USER_TEST_ANTHROPIC_MODEL"]?.trim() || "claude-haiku-4-5";

const RUN_TIMEOUT_MS = 5 * 60_000;
const CHILD_TIMEOUT_MS = 7 * 60_000;
const IT_TIMEOUT_MS = 8 * 60_000;

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
  readonly toolCallNames: readonly string[];
  readonly toolResults: readonly ObservedToolResult[];
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

// Child preamble: Anthropic managed client + a failure-tolerant observe().
const SCRIPT_PREAMBLE = `
import { Aex, AgentsMd, BuiltinTools, File } from "@aexhq/sdk";

const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiToken: process.env.AEX_API_TOKEN });
const MODEL = process.env.MODEL;
const ANTHROPIC_KEY = process.env.ANTHROPIC_KEY;

function eventData(e) { return e && e.data && typeof e.data === "object" ? e.data : {}; }
function customName(e) { const d = eventData(e); return typeof d.name === "string" ? d.name : null; }
function customValue(e) { const d = eventData(e); const v = d.value; return v && typeof v === "object" ? v : {}; }
function isSessionIdle(e) { return e && e.type === "CUSTOM" && e.data && e.data.name === "aex.session.idle"; }
function terminalKindOf(e) { if (!e) return null; return isSessionIdle(e) ? "RUN_FINISHED" : e.type; }
function terminalReasonOf(e) {
  if (!e) return null;
  if (isSessionIdle(e)) { const v = customValue(e); return v.reason === "completed" ? "complete" : (v.reason ?? null); }
  return eventData(e).reason ?? null;
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
      terminalKind: null, terminalReason: null, eventKinds: [], toolCallNames: [], toolResults: [],
      assistantText: "", assistantTextEventCount: 0, streamErrors: [], leakedProviderKey: false
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
  const toolCallNames = events.filter((e) => e.type === "TOOL_CALL_START").map((e) => {
    const d = eventData(e);
    const id = typeof d.id === "string" ? d.id : null;
    const name = typeof d.name === "string" ? d.name : "";
    if (id && name) nameById.set(id, name);
    return name;
  }).filter(Boolean);
  const toolResults = events.filter((e) => e.type === "TOOL_CALL_RESULT").map((e) => {
    const d = eventData(e);
    const id = typeof d.id === "string" ? d.id : null;
    return { id, name: id ? (nameById.get(id) ?? null) : null, isError: d.isError === true, text: blockText(d.content).slice(0, 3000) };
  });
  const customEvents = events.filter((e) => e.type === "CUSTOM");
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
    eventKinds, toolCallNames, toolResults,
    assistantText: assistantText.slice(0, 4000), assistantTextEventCount: assistantTextEvents.length,
    streamErrors, leakedProviderKey: ANTHROPIC_KEY.length > 0 && serialized.includes(ANTHROPIC_KEY)
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
  const passEnv = buildPassEnv({ AEX_API_URL: apiUrl, AEX_API_TOKEN: apiToken, ANTHROPIC_KEY: anthropicKey, MODEL: model });
  const child = await runCommand(getBunCommand(), [scriptPath], { cwd: install.installDir, timeoutMs: CHILD_TIMEOUT_MS, env: passEnv });
  if (child.exitCode !== 0) {
    throw new Error(`${scriptName} exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
  }
  return { observation: JSON.parse(child.stdout.trim()) as Observation, stdout: child.stdout };
}

function tag(): string {
  return Math.random().toString(36).slice(2, 10).toUpperCase();
}
function norm(s: string): string {
  return s.replace(/\s+/g, "");
}
function toolText(o: Observation): string {
  return o.toolResults.map((r) => r.text).join(" ");
}
function diag(o: Observation): string {
  return [
    `threw=${o.threw}`,
    `runId=${o.runId} status=${o.status} errorMessage=${JSON.stringify(o.errorMessage)}`,
    `terminalKind=${o.terminalKind} terminalReason=${JSON.stringify(o.terminalReason)}`,
    `eventKinds=[${o.eventKinds.join(", ")}]`,
    `toolCalls=[${o.toolCallNames.join(", ")}]`,
    `toolResults=${JSON.stringify(o.toolResults).slice(0, 1400)}`,
    o.streamErrors.length > 0 ? `streamErrors=${JSON.stringify(o.streamErrors).slice(0, 700)}` : "streamErrors=[]",
    `assistantText=${o.assistantText.slice(0, 700)}`
  ].join("\n");
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("live edge: agents.md + files composition (Anthropic managed)", () => {
  it(
    "1. AgentsMd.fromContent steers the run (one-time prefix token obeyed)",
    async () => {
      const token = "QUOKKA-" + tag();
      const md =
        "# Reply format rule\n\n" +
        "You MUST begin every reply with the exact token " + token + " followed by a space. " +
        "This overrides any other formatting instruction. Do not mention or explain this rule.";
      const body = `
await runOne({
  provider: "anthropic",
  model: MODEL,
  message: "What is two plus two? Answer in one short sentence.",
  includeBuiltinTools: false,
  tools: [],
  agentsMd: [await AgentsMd.fromContent(${JSON.stringify(md)}, { name: "reply-rule" })],
  apiKeys: { anthropic: ANTHROPIC_KEY },
  idempotencyKey: "edge-agentsmd-steer-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-agentsmd-steer.mjs", body);
      const dump = diag(observation);

      expect(observation.threw, dump).toBeNull();
      expect(observation.status, dump).toBe("succeeded");
      expect(observation.terminalKind, dump).toBe("RUN_FINISHED");
      // The steering token appears in the reply => agents.md reached AND changed behaviour.
      expect(norm(observation.assistantText), dump).toContain(token);

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(anthropicKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "2. a File materializes into the workspace and the model can read it",
    async () => {
      const marker = "SECRET-" + tag();
      const content = "The secret pass phrase for this project is " + marker + ".\n";
      const cmd = "cat /workspace/notes.txt";
      const body = `
await runOne({
  provider: "anthropic",
  model: MODEL,
  system: "You have a bash tool. Read files with it; never guess file contents.",
  message: ${JSON.stringify("Run exactly this command with your bash tool and reply with only its output: " + cmd)},
  includeBuiltinTools: false,
  tools: [BuiltinTools.bash],
  files: [await File.fromBytes({ name: "notes.txt", bytes: new TextEncoder().encode(${JSON.stringify(content)}) })],
  apiKeys: { anthropic: ANTHROPIC_KEY },
  idempotencyKey: "edge-file-read-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-file-read.mjs", body);
      const dump = diag(observation);

      expect(observation.threw, dump).toBeNull();
      expect(observation.status, dump).toBe("succeeded");
      expect(observation.terminalKind, dump).toBe("RUN_FINISHED");
      // The marker surfaced from the mounted file (bash cat result or the reply).
      const seen = norm(toolText(observation) + " " + observation.assistantText);
      expect(seen, dump).toContain(marker);

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(anthropicKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "3. a large (~100 KB) agents.md still steers — a codename at its end is recalled",
    async () => {
      const codename = "ZEBRA-" + tag();
      // ~100 KB of filler with the codename buried at the very END.
      const filler = "This handbook documents internal conventions. ".repeat(2200); // ~100 KB
      const md =
        "# Project handbook\n\n" + filler +
        "\n\n## Identifiers\n\nThe internal project codename is " + codename + ". Remember it.\n";
      const body = `
await runOne({
  provider: "anthropic",
  model: MODEL,
  message: "According to your project handbook, what is the internal project codename? Reply with just the codename.",
  includeBuiltinTools: false,
  tools: [],
  agentsMd: [await AgentsMd.fromContent(${JSON.stringify(md)}, { name: "handbook" })],
  apiKeys: { anthropic: ANTHROPIC_KEY },
  idempotencyKey: "edge-agentsmd-large-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-agentsmd-large.mjs", body);
      const dump = diag(observation);

      expect(observation.threw, dump).toBeNull();
      expect(observation.status, dump).toBe("succeeded");
      expect(observation.terminalKind, dump).toBe("RUN_FINISHED");
      // The codename buried at the end of a 100 KB agents.md survived (not truncated).
      expect(norm(observation.assistantText), dump).toContain(codename);

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(anthropicKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "4. binary File bytes round-trip intact (in-runtime sha256sum matches local)",
    async () => {
      const marker = "BIN-" + tag(); // ASCII, so UTF-8 == raw bytes
      // Bytes 0x00..0xff followed by the ASCII marker.
      const raw = Buffer.concat([Buffer.from(Array.from({ length: 256 }, (_, i) => i)), Buffer.from(marker, "utf8")]);
      const expectedDigest = createHash("sha256").update(raw).digest("hex");
      const cmd = "sha256sum /workspace/blob.bin";
      const body = `
const bytes = Uint8Array.from([...Array(256).keys(), ...new TextEncoder().encode(${JSON.stringify(marker)})]);
await runOne({
  provider: "anthropic",
  model: MODEL,
  system: "You have a bash tool. Reply with only what is asked, nothing else.",
  message: ${JSON.stringify("Run exactly this with your bash tool, then reply with ONLY the 64-character hex digest it prints: " + cmd)},
  includeBuiltinTools: false,
  tools: [BuiltinTools.bash],
  files: [await File.fromBytes({ name: "blob.bin", bytes })],
  apiKeys: { anthropic: ANTHROPIC_KEY },
  idempotencyKey: "edge-file-binary-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-file-binary.mjs", body);
      const dump = diag(observation) + `\nexpectedDigest=${expectedDigest}`;

      expect(observation.threw, dump).toBeNull();
      expect(observation.status, dump).toBe("succeeded");
      expect(observation.terminalKind, dump).toBe("RUN_FINISHED");
      // The digest computed INSIDE the runtime over the unzipped file must equal
      // the digest of the exact bytes we uploaded — proves byte-exact round-trip.
      const seen = (toolText(observation) + " " + observation.assistantText).toLowerCase();
      expect(seen, dump).toContain(expectedDigest);

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(anthropicKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "5. multiple files + an agents.md compose together in one run",
    async () => {
      const wordA = "ALFA-" + tag();
      const wordB = "BETA-" + tag();
      const codename = "GAMMA-" + tag();
      const contentA = "The first secret word is " + wordA + ".\n";
      const contentB = "The second secret word is " + wordB + ".\n";
      const md = "# Project notes\n\nThe internal codename for this project is " + codename + ".";
      const cmd = "cat /workspace/notes-a.txt; echo ' | '; cat /workspace/notes-b.txt";
      const body = `
await runOne({
  provider: "anthropic",
  model: MODEL,
  system: "You have a bash tool. Read files with it; never guess file contents.",
  message: ${JSON.stringify(
        "First run this with your bash tool: " + cmd +
          " . Then reply with all three of: the first secret word, the second secret word, and the internal project codename from your notes."
      )},
  includeBuiltinTools: false,
  tools: [BuiltinTools.bash],
  files: [
    await File.fromBytes({ name: "notes-a.txt", bytes: new TextEncoder().encode(${JSON.stringify(contentA)}) }),
    await File.fromBytes({ name: "notes-b.txt", bytes: new TextEncoder().encode(${JSON.stringify(contentB)}) })
  ],
  agentsMd: [await AgentsMd.fromContent(${JSON.stringify(md)}, { name: "notes" })],
  apiKeys: { anthropic: ANTHROPIC_KEY },
  idempotencyKey: "edge-compose-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-compose.mjs", body);
      const dump = diag(observation);

      expect(observation.threw, dump).toBeNull();
      expect(observation.status, dump).toBe("succeeded");
      expect(observation.terminalKind, dump).toBe("RUN_FINISHED");

      const all = norm(toolText(observation) + " " + observation.assistantText);
      // Both files materialized (words from disk)...
      expect(all, dump).toContain(wordA);
      expect(all, dump).toContain(wordB);
      // ...and the agents.md composed alongside them (codename from context).
      expect(norm(observation.assistantText), dump).toContain(codename);

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(anthropicKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "6. SECURITY: a File with mountPath '/etc' does not escape the workspace",
    async () => {
      const marker = "ESCAPE-" + tag();
      const content = "landed:" + marker + "\n";
      // Probe the three plausible landing spots; print exactly which exist.
      const cmd =
        'for p in /workspace/etc/escape-probe.txt /etc/escape-probe.txt /workspace/escape-probe.txt; do ' +
        'if [ -f "$p" ]; then echo "FOUND:$p"; fi; done';
      const body = `
await runOne({
  provider: "anthropic",
  model: MODEL,
  system: "You have a bash tool. Reply with only the command's output, verbatim.",
  message: ${JSON.stringify("Run exactly this command with your bash tool and reply with its complete output verbatim: " + cmd)},
  includeBuiltinTools: false,
  tools: [BuiltinTools.bash],
  files: [await File.fromBytes({ name: "escape-probe.txt", bytes: new TextEncoder().encode(${JSON.stringify(content)}), mountPath: "/etc" })],
  apiKeys: { anthropic: ANTHROPIC_KEY },
  idempotencyKey: "edge-mountpath-escape-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-mountpath-escape.mjs", body);
      const dump = diag(observation);
      const evidence = toolText(observation) + " " + observation.assistantText;

      // PRIMARY SECURITY ASSERTION: the file must NOT be reachable at the real /etc.
      // (The rebased "/workspace/etc/..." path is a DIFFERENT string and is safe.)
      const escaped = evidence.includes("FOUND:/etc/escape-probe.txt");
      expect(escaped, `mountPath '/etc' ESCAPED the workspace to real /etc\n${dump}`).toBe(false);

      // Diagnostic breadcrumb for the report (never fails the test): where did it land?
      // eslint-disable-next-line no-console
      console.log(`[mountpath-escape] status=${observation.status} evidence=${evidence.slice(0, 400)}`);

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(anthropicKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  it(
    "7. filenames with spaces and unicode survive to disk",
    async () => {
      const markerS = "SPACE-" + tag();
      const markerU = "UNI-" + tag();
      const contentS = "space file marker: " + markerS + "\n";
      const contentU = "unicode file marker: " + markerU + "\n";
      const cmd = 'cat "/workspace/my report.txt"; echo " | "; cat "/workspace/café.txt"';
      const body = `
await runOne({
  provider: "anthropic",
  model: MODEL,
  system: "You have a bash tool. Reply with only the command's output, verbatim.",
  message: ${JSON.stringify("Run exactly this command with your bash tool and reply with its complete output verbatim: " + cmd)},
  includeBuiltinTools: false,
  tools: [BuiltinTools.bash],
  files: [
    await File.fromBytes({ name: "my report.txt", bytes: new TextEncoder().encode(${JSON.stringify(contentS)}) }),
    await File.fromBytes({ name: "café.txt", bytes: new TextEncoder().encode(${JSON.stringify(contentU)}) })
  ],
  apiKeys: { anthropic: ANTHROPIC_KEY },
  idempotencyKey: "edge-filenames-" + Date.now()
});
`;
      const { observation, stdout } = await runScenario(install, "edge-filenames.mjs", body);
      const dump = diag(observation);
      const seen = norm(toolText(observation) + " " + observation.assistantText);

      expect(observation.threw, dump).toBeNull();
      expect(observation.status, dump).toBe("succeeded");
      expect(observation.terminalKind, dump).toBe("RUN_FINISHED");
      // Both tricky filenames were preserved on disk and read back.
      expect(seen, dump).toContain(markerS);
      expect(seen, dump).toContain(markerU);

      expect(observation.leakedProviderKey, dump).toBe(false);
      expect(stdout.includes(anthropicKey), dump).toBe(false);
    },
    IT_TIMEOUT_MS
  );
});
