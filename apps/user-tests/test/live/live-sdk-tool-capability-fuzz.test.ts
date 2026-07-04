/**
 * Explicit live USER fuzz gate for the complete agent-tool surface.
 *
 * This suite drives a clean install of the packed/published SDK, uploads real
 * files and custom tool bundles, submits real DeepSeek runs, and asserts the
 * hosted runner invoked every requested tool with deterministic results.
 *
 * The generated cases are seeded so a failure is reproducible. The suite is
 * intentionally excluded from the default live sweep because it creates
 * multiple paid runs; invoke it through `test:user:tool-fuzz`.
 */
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import fc from "fast-check";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  getBunCommand,
  installAex,
  runCommand,
  type InstallResult
} from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`user-tests live (tool-capability-fuzz): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

const FUZZ_CASES = 2;
const BASE_SEED = 0x0ae2026;
const LIVE_TIMEOUT_MS = 12 * 60_000;
const CHILD_TIMEOUT_MS = 15 * 60_000;

const wordArb = fc
  .array(fc.constantFrom(..."abcdefghijkmnpqrstuvwxyz"), { minLength: 4, maxLength: 9 })
  .map((chars) => chars.join(""));
const wordsArb = fc.array(wordArb, { minLength: 7, maxLength: 10 });

interface FileCase {
  readonly id: number;
  readonly lines: readonly string[];
  readonly needle: string;
  readonly replacement: string;
  readonly headLines: number;
  readonly tailLines: number;
}

const FILE_CASES: readonly FileCase[] = fc.sample(
  fc.record({
    lines: wordsArb,
    needle: wordArb.map((word) => `needle_${word}`),
    replacement: wordArb.map((word) => `replacement_${word}`),
    headLines: fc.integer({ min: 1, max: 3 }),
    tailLines: fc.integer({ min: 1, max: 3 })
  }),
  { seed: BASE_SEED + 1, numRuns: FUZZ_CASES }
).map((value, id) => ({ ...value, id }));

interface ProcessCase {
  readonly id: number;
  readonly left: number;
  readonly right: number;
  readonly todoWords: readonly string[];
  readonly commitWord: string;
}

const PROCESS_CASES: readonly ProcessCase[] = fc.sample(
  fc.record({
    left: fc.integer({ min: 2, max: 40 }),
    right: fc.integer({ min: 2, max: 40 }),
    todoWords: fc.array(wordArb, { minLength: 2, maxLength: 4 }),
    commitWord: wordArb
  }),
  { seed: BASE_SEED + 2, numRuns: FUZZ_CASES }
).map((value, id) => ({ ...value, id }));

interface BackgroundCase {
  readonly id: number;
  readonly marker: string;
}

const BACKGROUND_CASES: readonly BackgroundCase[] = fc.sample(
  wordArb.map((word) => ({ marker: `bg_${word}` })),
  { seed: BASE_SEED + 3, numRuns: FUZZ_CASES }
).map((value, id) => ({ ...value, id }));

interface WebCase {
  readonly id: number;
  readonly query: string;
  readonly terms: readonly string[];
  readonly maxResults: number;
  readonly maxBytes: number;
}

const WEB_CASES: readonly WebCase[] = fc.sample(
  fc.record({
    query: fc.constantFrom("Wikipedia free encyclopedia", "IANA example domains"),
    maxResults: fc.integer({ min: 1, max: 3 }),
    maxBytes: fc.integer({ min: 3500, max: 6000 })
  }),
  { seed: BASE_SEED + 4, numRuns: FUZZ_CASES }
).map((value, id) => ({
  ...value,
  id,
  terms: value.query.startsWith("Wikipedia")
    ? ["wikipedia", "encyclopedia"]
    : ["iana", "domain"]
}));

interface SubagentCase {
  readonly id: number;
  readonly marker: string;
}

const SUBAGENT_CASES: readonly SubagentCase[] = fc.sample(
  wordArb.map((word) => ({ marker: `child_${word}` })),
  { seed: BASE_SEED + 5, numRuns: FUZZ_CASES }
).map((value, id) => ({ ...value, id }));

interface CustomCase {
  readonly id: number;
  readonly text: string;
  readonly count: number;
  readonly enabled: boolean;
  readonly tags: readonly string[];
  readonly appMode: string;
}

const CUSTOM_CASES: readonly CustomCase[] = fc.sample(
  fc.record({
    text: wordArb,
    count: fc.integer({ min: 1, max: 4 }),
    enabled: fc.boolean(),
    tags: fc.array(wordArb, { minLength: 1, maxLength: 3 }),
    appMode: wordArb.map((word) => `mode_${word}`)
  }),
  { seed: BASE_SEED + 6, numRuns: FUZZ_CASES }
).map((value, id) => ({ ...value, id }));

interface ToolCall {
  readonly id: string | null;
  readonly name: string;
  readonly arguments: Readonly<Record<string, unknown>>;
}

interface ToolResult {
  readonly id: string | null;
  readonly name: string | null;
  readonly isError: boolean;
  readonly text: string;
}

interface OutputSample {
  readonly filename: string | null;
  readonly sizeBytes: number;
  readonly text: string | null;
}

interface Observation {
  readonly runId: string;
  readonly status: string;
  readonly eventKinds: readonly string[];
  readonly terminalKind: string | null;
  readonly terminalData: Readonly<Record<string, unknown>> | null;
  readonly assistantText: string;
  readonly toolCalls: readonly ToolCall[];
  readonly toolResults: readonly ToolResult[];
  readonly outputs: readonly OutputSample[];
}

const SCRIPT_PREAMBLE = `
import {
  Aex,
  BuiltinTools,
  File,
  Secret,
  Tool
} from "@aexhq/sdk";

const client = new Aex({
  baseUrl: process.env.AEX_API_URL,
  apiKey: process.env.AEX_API_KEY
});
const MODEL = process.env.MODEL_DEEPSEEK;
const DEEPSEEK_KEY = process.env.DEEPSEEK_KEY;

function eventData(event) {
  return event && event.data && typeof event.data === "object" ? event.data : {};
}

function isSessionIdle(event) {
  return event && event.type === "CUSTOM" && event.data && event.data.name === "aex.session.idle";
}

function terminalKindOf(event) {
  if (!event) return null;
  return isSessionIdle(event) ? "RUN_FINISHED" : event.type;
}

function terminalDataOf(event) {
  if (!event) return null;
  if (isSessionIdle(event)) {
    const value = event.data && event.data.value && typeof event.data.value === "object" ? event.data.value : {};
    return { ...value, reason: value.reason === "completed" ? "complete" : value.reason };
  }
  return eventData(event);
}

function blockText(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((block) => {
      if (typeof block === "string") return block;
      return block && typeof block.text === "string" ? block.text : "";
    })
    .join("\\n");
}

async function observe(result) {
  const fallbackEvents = Array.isArray(result.events) ? result.events : [];
  const fallbackOutputs = Array.isArray(result.outputs) ? result.outputs : [];
  const session = await client.sessions.open(result.runId);
  let events = fallbackEvents;
  let outputs = fallbackOutputs;
  try {
    const listedEvents = await session.events().list();
    if (Array.isArray(listedEvents) && listedEvents.length > 0) events = listedEvents;
    const listedOutputs = await session.outputs().list();
    if (Array.isArray(listedOutputs)) outputs = listedOutputs;
  } catch {
    events = fallbackEvents;
    outputs = fallbackOutputs;
  }
  const starts = events.filter((event) => event.type === "TOOL_CALL_START");
  const nameById = new Map();
  const toolCalls = starts.map((event) => {
    const data = eventData(event);
    const id = typeof data.id === "string" ? data.id : null;
    const name = typeof data.name === "string" ? data.name : "";
    if (id && name) nameById.set(id, name);
    const args =
      data.arguments && typeof data.arguments === "object" && !Array.isArray(data.arguments)
        ? data.arguments
        : {};
    return { id, name, arguments: args };
  });
  const toolResults = events
    .filter((event) => event.type === "TOOL_CALL_RESULT")
    .map((event) => {
      const data = eventData(event);
      const id = typeof data.id === "string" ? data.id : null;
      return {
        id,
        name: id ? (nameById.get(id) ?? null) : null,
        isError: data.isError === true,
        text: blockText(data.content)
      };
    });
  const terminal = events.find(
    (event) => event.type === "RUN_FINISHED" || event.type === "RUN_ERROR"
  ) ?? events.find(isSessionIdle);
  const eventKinds = events.map((event) => event.type);
  if (terminal && isSessionIdle(terminal) && !eventKinds.includes("RUN_FINISHED")) {
    eventKinds.push("RUN_FINISHED");
  }
  const outputSamples = [];
  for (const output of outputs.slice(0, 24)) {
    let text = null;
    try {
      const bytes = await session.outputs().download(output);
      text = new TextDecoder().decode(bytes).slice(0, 16_384);
    } catch (error) {
      text = "(download failed: " + (error && error.message ? error.message : String(error)) + ")";
    }
    outputSamples.push({
      filename: typeof output.filename === "string" ? output.filename : null,
      sizeBytes: typeof output.sizeBytes === "number" ? output.sizeBytes : 0,
      text
    });
  }
  return {
    runId: result.runId,
    status: result.ok
      ? "succeeded"
      : (typeof result.status === "string" && result.status ? result.status : "failed"),
    eventKinds,
    terminalKind: terminalKindOf(terminal),
    terminalData: terminalDataOf(terminal),
    assistantText: typeof result.text === "string" ? result.text : "",
    toolCalls,
    toolResults,
    outputs: outputSamples
  };
}
`;

function buildScript(body: string): string {
  return `${SCRIPT_PREAMBLE}\n${body}\n`;
}

function passEnv(extras: Readonly<Record<string, string>> = {}): Record<string, string> {
  const env: Record<string, string> = {
    AEX_API_URL: apiUrl,
    AEX_API_KEY: apiKey,
    DEEPSEEK_KEY: deepseekKey,
    MODEL_DEEPSEEK: deepseekModel,
    ...extras
  };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  const carry =
    process.platform === "win32"
      ? [
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
        ]
      : ["HOME", "TMPDIR", "LANG", "LC_ALL"];
  for (const key of carry) {
    if (process.env[key]) env[key] = process.env[key]!;
  }
  return env;
}

async function runScenario(
  install: InstallResult,
  scriptName: string,
  body: string,
  extras: Readonly<Record<string, string>> = {}
): Promise<{ readonly observation: Observation; readonly stdout: string }> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, buildScript(body));
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    env: passEnv(extras),
    timeoutMs: CHILD_TIMEOUT_MS
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `${scriptName} exited ${child.exitCode}\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return {
    observation: JSON.parse(child.stdout.trim()) as Observation,
    stdout: child.stdout
  };
}

function names(observation: Observation): readonly string[] {
  return observation.toolCalls.map((call) => call.name).filter(Boolean);
}

function allObservedText(observation: Observation): string {
  return [
    observation.assistantText,
    ...observation.toolResults.map((result) => result.text),
    ...observation.outputs.map((output) => output.text ?? "")
  ].join("\n");
}

function diagnostics(observation: Observation): string {
  return [
    `runId=${observation.runId}`,
    `status=${observation.status}`,
    `terminal=${observation.terminalKind} ${JSON.stringify(observation.terminalData)}`,
    `events=[${observation.eventKinds.join(",")}]`,
    `calls=${JSON.stringify(observation.toolCalls).slice(0, 3000)}`,
    `results=${JSON.stringify(observation.toolResults).slice(0, 4000)}`,
    `outputs=${JSON.stringify(observation.outputs.map((output) => ({
      filename: output.filename,
      sizeBytes: output.sizeBytes,
      text: output.text?.slice(0, 300)
    }))).slice(0, 3000)}`,
    `assistant=${observation.assistantText.slice(0, 1200)}`
  ].join("\n");
}

function assertToolSurface(
  observation: Observation,
  expectedNames: readonly string[],
  stdout: string,
  extraSecrets: readonly string[] = []
): void {
  const dump = diagnostics(observation);
  expect(observation.status, dump).toBe("succeeded");
  expect(observation.terminalKind, dump).toBe("RUN_FINISHED");
  expect(observation.terminalData?.["reason"], dump).toBe("complete");
  const called = names(observation);
  for (const expected of expectedNames) {
    if (!called.includes(expected)) {
      throw new Error(`expected tool ${expected}, got [${called.join(", ")}]\n${dump}`);
    }
  }
  expect(stdout.includes(deepseekKey), dump).toBe(false);
  for (const secret of extraSecrets) {
    expect(stdout.includes(secret), dump).toBe(false);
  }
}

function outputEnding(observation: Observation, suffix: string): OutputSample | undefined {
  return observation.outputs.find((output) => (output.filename ?? "").endsWith(suffix));
}

function sha256(value: string): string {
  return createHash("sha256").update(value, "utf8").digest("hex");
}

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("live installed SDK tool capability fuzz", () => {
  it.each(FILE_CASES)(
    "seeded file/navigation case $id covers read/write/edit/grep/glob/head/tail",
    async (testCase) => {
      const sourceLines = [...testCase.lines];
      sourceLines[2] = `${sourceLines[2]} ${testCase.needle}`;
      const sourceText = `${sourceLines.join("\n")}\n`;
      const generatedBefore = `alpha\n${testCase.needle}\nomega\n`;
      const generatedAfter = `alpha\n${testCase.replacement}\nomega\n`;
      const prompt = [
        "Use only the named tools and perform every step in order.",
        "1. read_file fuzz/source.txt in full.",
        `2. write_file fuzz/generated.txt with exactly: ${JSON.stringify(generatedBefore)}.`,
        "3. read_file fuzz/generated.txt in full.",
        `4. edit_file fuzz/generated.txt, replacing ${testCase.needle} with ${testCase.replacement}.`,
        `5. grep for ${testCase.needle} under fuzz with max_results=10.`,
        "6. glob fuzz/**/*.txt.",
        `7. head fuzz/source.txt with lines=${testCase.headLines}.`,
        `8. tail fuzz/source.txt with lines=${testCase.tailLines}.`,
        `Reply with one line containing FILE_FUZZ_OK ${testCase.needle} ${testCase.replacement}.`
      ].join("\n");
      const body = `
const source = await File.fromBytes({
  name: "source.txt",
  bytes: new TextEncoder().encode(${JSON.stringify(sourceText)}),
  mountPath: "/fuzz"
});
const other = await File.fromBytes({
  name: "other.md",
  bytes: new TextEncoder().encode("distractor only\\n"),
  mountPath: "/fuzz"
});
const result = await client.run({
  provider: "deepseek",
  model: MODEL,
  message: ${JSON.stringify(prompt)},
  files: [source, other],
  includeBuiltinTools: false,
  tools: [
    BuiltinTools.read_file,
    BuiltinTools.write_file,
    BuiltinTools.edit_file,
    BuiltinTools.grep,
    BuiltinTools.glob,
    BuiltinTools.head,
    BuiltinTools.tail
  ],
  outputs: { allowedDirs: ["/workspace/fuzz"] },
  apiKeys: { deepseek: DEEPSEEK_KEY },
  idempotencyKey: "tool-fuzz-files-${testCase.id}-" + Date.now()
}, { timeoutMs: ${LIVE_TIMEOUT_MS} });
process.stdout.write(JSON.stringify(await observe(result)));
`;
      const { observation, stdout } = await runScenario(
        install,
        `tool-fuzz-files-${testCase.id}.mjs`,
        body
      );
      assertToolSurface(
        observation,
        ["read_file", "write_file", "edit_file", "grep", "glob", "head", "tail"],
        stdout
      );
      const dump = diagnostics(observation);
      const generated = outputEnding(observation, "generated.txt");
      expect(generated?.text, dump).toBe(generatedAfter);
      const text = allObservedText(observation);
      expect(text, dump).toContain(testCase.needle);
      expect(text, dump).toContain(testCase.replacement);
      expect(text, dump).toContain(sourceLines[0]!);
      expect(text, dump).toContain(sourceLines.at(-1)!);
    },
    16 * 60_000
  );

  it.each(PROCESS_CASES)(
    "seeded process/state case $id covers bash/code_execution/todo_write/wait/git",
    async (testCase) => {
      const bashMarker = `bash_${testCase.commitWord}`;
      const pyValue = testCase.left * testCase.right;
      const jsValue = testCase.left + testCase.right;
      const commitSubject = `commit-${testCase.commitWord}`;
      const todos = testCase.todoWords.map((word, index) => ({
        content: `todo ${word}`,
        status: index === 0 ? "completed" : index === 1 ? "in_progress" : "pending",
        activeForm: `doing ${word}`
      }));
      const prompt = [
        "Use only the named tools and perform every step.",
        `1. bash: printf '${bashMarker}\\n' and create note.txt containing ${bashMarker}.`,
        `2. code_execution in python: print(${testCase.left} * ${testCase.right}).`,
        `3. code_execution in javascript: console.log(${testCase.left} + ${testCase.right}).`,
        `4. todo_write exactly this full JSON list: ${JSON.stringify(todos)}.`,
        "5. wait with seconds=1.",
        "6. Using only the git tool for git commands: init, configure user.email and user.name, add note.txt,",
        `   commit it with subject ${commitSubject}, then run log --oneline -1.`,
        `Reply with PROCESS_FUZZ_OK ${bashMarker} py=${pyValue} js=${jsValue} ${commitSubject}.`
      ].join("\n");
      const body = `
const result = await client.run({
  provider: "deepseek",
  model: MODEL,
  message: ${JSON.stringify(prompt)},
  includeBuiltinTools: false,
  tools: [
    BuiltinTools.bash,
    BuiltinTools.code_execution,
    BuiltinTools.todo_write,
    BuiltinTools.wait,
    BuiltinTools.git
  ],
  outputs: { allowedDirs: ["/workspace/.aex"] },
  apiKeys: { deepseek: DEEPSEEK_KEY },
  idempotencyKey: "tool-fuzz-process-${testCase.id}-" + Date.now()
}, { timeoutMs: ${LIVE_TIMEOUT_MS} });
process.stdout.write(JSON.stringify(await observe(result)));
`;
      const { observation, stdout } = await runScenario(
        install,
        `tool-fuzz-process-${testCase.id}.mjs`,
        body
      );
      assertToolSurface(
        observation,
        ["bash", "code_execution", "todo_write", "wait", "git"],
        stdout
      );
      const dump = diagnostics(observation);
      const text = allObservedText(observation);
      expect(text, dump).toContain(bashMarker);
      expect(text, dump).toContain(String(pyValue));
      expect(text, dump).toContain(String(jsValue));
      expect(text, dump).toContain(commitSubject);
      const languages = observation.toolCalls
        .filter((call) => call.name === "code_execution")
        .map((call) => call.arguments["language"]);
      expect(languages, dump).toContain("python");
      expect(languages, dump).toContain("javascript");
      const todoOutput = outputEnding(observation, "todos.json");
      expect(todoOutput, dump).toBeDefined();
      expect(JSON.parse(todoOutput?.text ?? "null"), dump).toEqual(todos);
    },
    16 * 60_000
  );

  it.each(BACKGROUND_CASES)(
    "seeded background case $id covers bash background/output/kill",
    async (testCase) => {
      const prompt = [
        "Use only the named tools.",
        "1. Call bash with run_in_background=true for this exact command:",
        `   for i in $(seq 1 120); do echo ${testCase.marker}_tick_$i; sleep 1; done`,
        "2. Call bash_output with the returned job id and report a tick you observed.",
        "3. Call bash_kill with the same job id.",
        `Reply with BG_FUZZ_OK ${testCase.marker}.`
      ].join("\n");
      const body = `
const result = await client.run({
  provider: "deepseek",
  model: MODEL,
  message: ${JSON.stringify(prompt)},
  includeBuiltinTools: false,
  tools: [BuiltinTools.bash, BuiltinTools.bash_output, BuiltinTools.bash_kill],
  apiKeys: { deepseek: DEEPSEEK_KEY },
  idempotencyKey: "tool-fuzz-bg-${testCase.id}-" + Date.now()
}, { timeoutMs: ${LIVE_TIMEOUT_MS} });
process.stdout.write(JSON.stringify(await observe(result)));
`;
      const { observation, stdout } = await runScenario(
        install,
        `tool-fuzz-bg-${testCase.id}.mjs`,
        body
      );
      assertToolSurface(observation, ["bash", "bash_output", "bash_kill"], stdout);
      const dump = diagnostics(observation);
      const backgroundStart = observation.toolCalls.find(
        (call) =>
          call.name === "bash" &&
          call.arguments["run_in_background"] === true
      );
      expect(backgroundStart, dump).toBeDefined();
      expect(allObservedText(observation), dump).toContain(`${testCase.marker}_tick_`);
    },
    16 * 60_000
  );

  it.each(WEB_CASES)(
    "seeded web case $id covers web_fetch extraction and web_search result bounds",
    async (testCase) => {
      const prompt = [
        "Use only the named tools.",
        `1. web_fetch https://example.com with max_bytes=${testCase.maxBytes} and prompt "extract the main heading".`,
        `2. web_search for ${JSON.stringify(testCase.query)} with max_results=${testCase.maxResults}.`,
        `Reply with WEB_FUZZ_OK, the fetched heading, and a short search summary.`
      ].join("\n");
      const body = `
const result = await client.run({
  provider: "deepseek",
  model: MODEL,
  message: ${JSON.stringify(prompt)},
  includeBuiltinTools: false,
  tools: [BuiltinTools.web_fetch, BuiltinTools.web_search],
  apiKeys: { deepseek: DEEPSEEK_KEY },
  idempotencyKey: "tool-fuzz-web-${testCase.id}-" + Date.now()
}, { timeoutMs: ${LIVE_TIMEOUT_MS} });
process.stdout.write(JSON.stringify(await observe(result)));
`;
      const { observation, stdout } = await runScenario(
        install,
        `tool-fuzz-web-${testCase.id}.mjs`,
        body
      );
      assertToolSurface(observation, ["web_fetch", "web_search"], stdout);
      const dump = diagnostics(observation);
      const text = allObservedText(observation).toLowerCase();
      expect(text, dump).toContain("example domain");
      for (const term of testCase.terms) {
        expect(text, dump).toContain(term);
      }
      const fetchCall = observation.toolCalls.find((call) => call.name === "web_fetch");
      const searchCall = observation.toolCalls.find((call) => call.name === "web_search");
      expect(fetchCall?.arguments["max_bytes"], dump).toBe(testCase.maxBytes);
      expect(searchCall?.arguments["max_results"], dump).toBe(testCase.maxResults);
    },
    16 * 60_000
  );

  it.each(SUBAGENT_CASES)(
    "seeded subagent case $id covers async spawn and subagent_result collection",
    async (testCase) => {
      const childFile = `${testCase.marker}.txt`;
      const childPrompt = [
        `Use write_file to create /workspace/${childFile}.`,
        `The file must contain exactly ${testCase.marker}.`,
        "Then finish."
      ].join(" ");
      const prompt = [
        "Use only the named tools.",
        `1. Call subagent exactly once with model=${deepseekModel}, includeBuiltinTools=false,`,
        `   tools=["write_file"], and prompt=${JSON.stringify(childPrompt)}.`,
        "2. Take the returned child run id and call subagent_result.",
        "3. If terminal is false, call subagent_result again with the same id until terminal is true.",
        `4. Confirm the output list contains ${childFile}.`,
        `Reply with SUBAGENT_FUZZ_OK ${testCase.marker}.`
      ].join("\n");
      const body = `
const result = await client.run({
  provider: "deepseek",
  model: MODEL,
  message: ${JSON.stringify(prompt)},
  includeBuiltinTools: false,
  tools: [BuiltinTools.subagent, BuiltinTools.subagent_result],
  apiKeys: { deepseek: DEEPSEEK_KEY },
  idempotencyKey: "tool-fuzz-subagent-${testCase.id}-" + Date.now()
}, { timeoutMs: ${LIVE_TIMEOUT_MS} });
process.stdout.write(JSON.stringify(await observe(result)));
`;
      const { observation, stdout } = await runScenario(
        install,
        `tool-fuzz-subagent-${testCase.id}.mjs`,
        body
      );
      assertToolSurface(observation, ["subagent", "subagent_result"], stdout);
      const dump = diagnostics(observation);
      const resultText = observation.toolResults
        .filter((result) => result.name === "subagent_result")
        .map((result) => result.text)
        .join("\n");
      expect(resultText, dump).toContain('"terminal":true');
      expect(resultText, dump).toContain(childFile);
    },
    20 * 60_000
  );

  it.each(CUSTOM_CASES)(
    "seeded custom-tool case $id covers schemas, context, result forms, errors, and redaction",
    async (testCase) => {
      const customSecret = `custom-secret-${testCase.id}-${testCase.text}-${testCase.count}`;
      const secretDigest = sha256(customSecret);
      const expectedTransform = [
        testCase.text.toUpperCase(),
        String(testCase.count),
        String(testCase.enabled),
        testCase.tags.join(",")
      ].join("|");
      const transformInput = {
        text: testCase.text,
        count: testCase.count,
        enabled: testCase.enabled,
        tags: testCase.tags
      };
      const prompt = [
        "Use every custom tool exactly once, in this order.",
        `1. custom_transform with ${JSON.stringify(transformInput)}.`,
        "2. custom_context with an empty object.",
        `3. custom_failure with {"reason":"expected_${testCase.text}"}. This failure is expected; continue.`,
        `Reply with CUSTOM_FUZZ_OK ${expectedTransform} ${testCase.appMode} ${secretDigest}.`
      ].join("\n");
      const body = `
const transform = await Tool.fromFiles({
  name: "custom_transform",
  description: "Transforms a structured fuzz input exactly.",
  inputSchema: {
    type: "object",
    properties: {
      text: { type: "string" },
      count: { type: "integer", minimum: 1, maximum: 4 },
      enabled: { type: "boolean" },
      tags: { type: "array", items: { type: "string" }, minItems: 1, maxItems: 3 }
    },
    required: ["text", "count", "enabled", "tags"],
    additionalProperties: false
  },
  entry: "index.mjs",
  files: {
    "index.mjs": [
      "export default async function ({ input }) {",
      "  return [String(input.text).toUpperCase(), input.count, input.enabled, input.tags.join(',')].join('|');",
      "}"
    ].join("\\n")
  }
});
const context = await Tool.fromFiles({
  name: "custom_context",
  description: "Reads customer environment and secret context with redaction.",
  inputSchema: {
    type: "object",
    properties: {},
    required: [],
    additionalProperties: false
  },
  entry: "index.mjs",
  files: {
    "index.mjs": [
      "import { createHash } from 'node:crypto';",
      "export default {",
      "  async execute({ aex }) {",
      "    const mode = await aex.env.get_value('APP_MODE');",
      "    const secret = await aex.secrets.get_value('CUSTOM_SECRET');",
      "    const digest = createHash('sha256').update(secret).digest('hex');",
      "    return { content: [{ type: 'text', text: 'mode=' + mode + ' digest=' + digest + ' raw=' + secret }] };",
      "  }",
      "};"
    ].join("\\n")
  }
});
const failure = await Tool.fromFiles({
  name: "custom_failure",
  description: "Throws an expected error after reading a secret.",
  inputSchema: {
    type: "object",
    properties: { reason: { type: "string" } },
    required: ["reason"],
    additionalProperties: false
  },
  entry: "index.mjs",
  files: {
    "index.mjs": [
      "export async function execute({ input, aex }) {",
      "  const secret = await aex.secrets.get_value('CUSTOM_SECRET');",
      "  throw new Error(input.reason + ' secret=' + secret);",
      "}"
    ].join("\\n")
  }
});
const result = await client.run({
  provider: "deepseek",
  model: MODEL,
  message: ${JSON.stringify(prompt)},
  includeBuiltinTools: false,
  tools: [transform, context, failure],
  environment: {
    variables: { APP_MODE: ${JSON.stringify(testCase.appMode)} },
    secrets: { CUSTOM_SECRET: Secret.value(process.env.CUSTOM_SECRET) }
  },
  apiKeys: { deepseek: DEEPSEEK_KEY },
  idempotencyKey: "tool-fuzz-custom-${testCase.id}-" + Date.now()
}, { timeoutMs: ${LIVE_TIMEOUT_MS} });
process.stdout.write(JSON.stringify(await observe(result)));
`;
      const { observation, stdout } = await runScenario(
        install,
        `tool-fuzz-custom-${testCase.id}.mjs`,
        body,
        { CUSTOM_SECRET: customSecret }
      );
      assertToolSurface(
        observation,
        ["custom_transform", "custom_context", "custom_failure"],
        stdout,
        [customSecret]
      );
      const dump = diagnostics(observation);
      const text = allObservedText(observation);
      expect(text, dump).toContain(expectedTransform);
      expect(text, dump).toContain(testCase.appMode);
      expect(text, dump).toContain(secretDigest);
      const transformCall = observation.toolCalls.find(
        (call) => call.name === "custom_transform"
      );
      expect(transformCall?.arguments, dump).toEqual(transformInput);
      const failureResult = observation.toolResults.find(
        (result) => result.name === "custom_failure"
      );
      expect(failureResult?.isError, dump).toBe(true);
      expect(failureResult?.text, dump).toContain(`expected_${testCase.text}`);
      expect(failureResult?.text.includes(customSecret), dump).toBe(false);
      expect(/REDACTED|\*{3}/.test(failureResult?.text ?? ""), dump).toBe(true);
    },
    16 * 60_000
  );
});
