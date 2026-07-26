/**
 * Live scenario: live-sdk-heavy-session.test.ts
 *
 * The "heavy" full-feature long-session suite. Where
 * live-sdk-comprehensive.test.ts proves each cell works with a short
 * run, THIS suite submits one deliberately heavy, multi-minute session
 * per cell that exercises the *entire* customer feature surface at once
 * and validates EVERY observable aspect of the session. The goal is not to
 * test the model's capability — it is to prove the aex app
 * (materialization, managed gateway, event log, files pipeline, secret
 * caps) behaves as expected under a maximal submission.
 *
 * Sessions as an explicit gate AFTER the rest of the live user-tests pass
 * (own Bun script `test:user:heavy`, its own dedicated lane), so it is
 * never swept into the default `test:user` run. Wired into
 * live-user-tests.yml as the manual canary.
 *
 * Scope: one DeepSeek-managed cell, fanned out across runtime kinds.
 *
 * Each cell submits ONE run carrying the full surface together:
 *   - 3 inline Skills          multi-skill manifest + materialization
 *   - 2 remote MCP servers      submission accepts a multi-MCP payload
 *                               (tool INVOCATION is proven by
 *                               live-sdk-mcp-invocation.test.ts, not here —
 *                               this prompt does not steer the model to the MCP)
 *   - a long multi-paragraph `system` message (probe-tagged)
 *   - a multi-step `prompt` array (forces shell + multiple file WRITES +
 *                                  multiple file READS, then a probe ack)
 *   - 1 instructions resource   (probe-tagged project guidance)
 *   - a custom `fileCapture.allowedDirs` path (proves re-rooting under workspaceRoot)
 *   - `builtins: ["developer"]`, `environment.envVars`, `metadata`
 *   - no customer-supplied provider credentials
 *
 * Input files / workspace assets are intentionally NOT exercised — that
 * feature was dropped in the MVP for simplicity. "Read file" coverage
 * comes from the agent reading back the files it writes, on the
 * supported developer-tool path.
 *
 * Managed cells (full assertion set — "validate all aspects"):
 *   - run outcome reached `succeeded`; runtime/provider echo back correctly.
 *   - each run is framed by RUN_STARTED ... RUN_FINISHED.
 *   - FULL EVENT VOCABULARY: the distinct event types observed are a
 *     superset of every event type required for a successful managed session:
 *     TEXT_MESSAGE_CONTENT, TOOL_CALL_START, TOOL_CALL_RESULT, CUSTOM
 *     RUN_ERROR is failure-only and is covered by
 *     live-sdk-files-and-failures.test.ts.
 *     See AEX_EVENT_TYPES
 *     in packages/contracts/src/event-envelope.ts — the single source of
 *     truth this list is kept in sync with.
 *   - the agent actually used tools (TOOL_CALL_START/RESULT counts > 0).
 *   - all 3 submitted skills produced a `skill_loaded` notification (every
 *     skill materialized into the container).
 *   - system + instructions + prompt probes all appear in the SDK-visible event
 *     transcript (assistant text, tool-call arguments, or tool results), proving
 *     composeInstructions carried all three channels into the recipe.
 *   - the FILES pipeline round-trips: every captured file downloads
 *     without error, and at least one agent-written file carries its
 *     expected REF-out token (write → capture → object storage → download e2e).
 *   - the workspace API key never appears anywhere in the
 *     SDK-visible payload (run + events + files).
 *
 * Required env:
 *   AEX_API_URL                live hosted API URL (local or prod)
 *   AEX_API_KEY               workspace API key
 *   AEX_USER_TEST_TARBALL            packed SDK tarball
 *     OR AEX_USER_TEST_VERSION       published package version
 * Optional:
 *   AEX_USER_TEST_DEEPSEEK_MODEL    default "deepseek/deepseek-v4-flash"
 *   AEX_USER_TEST_RUNTIME          default "spot_container"
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { assertManagedShape, type CaseResult, type Probes } from "../_fixtures/heavy-session-shape.js";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { finishedRunReadinessSource } from "../_fixtures/finished-run-readiness.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (heavy-session): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek/deepseek-v4-flash";
const runtimeKind = selectedRuntimeKind();

function selectedRuntimeKind(): "container" | "spot_container" | "lambda" {
  const value = process.env["AEX_USER_TEST_RUNTIME"]?.trim() || "spot_container";
  if (value !== "container" && value !== "spot_container" && value !== "lambda") {
    throw new Error(`user-tests live (heavy-session): invalid AEX_USER_TEST_RUNTIME ${JSON.stringify(value)}.`);
  }
  return value;
}

// DeepWiki MCP — public, unauthenticated, exposes GitHub repo Q&A tools.
const MCP_SERVER_URL = "https://mcp.deepwiki.com/mcp";
const MCP_SERVER_NAME = "deepwiki";

// Custom explicit files path. Must be BOTH agent-writable and
// captured by fileCapture.allowedDirs: the runner's resolveInsideWorkspace re-roots
// allowed dirs under workspaceRoot (/workspace) and dedupes a leading
// `workspace/` segment, so "/workspace/files/heavy" captures exactly
// the path the agent writes to. (An arbitrary path like /data/... gets
// captured at <root>/data/... but the agent can't write to literal
// /data — its writable tree is /workspace.)
const CUSTOM_OUTPUT_DIR = "/workspace/files/heavy";

function managedHeavySkillName(role: "alpha" | "beta" | "gamma", provider: CaseSpec["provider"]): string {
  return `heavy-${role}-managed-${provider}`;
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

interface CaseSpec {
  readonly scriptName: string;
  readonly provider: "deepseek";
  readonly model: string;
  readonly pollDeadlineMs: number;
  readonly pollIntervalMs: number;
  readonly timeoutMs: number;
}

function buildScript(spec: CaseSpec, probes: Probes): string {
  // Probe framing follows live-sdk-comprehensive: tokens are "tracking
  // references" (mundane, audit-flavoured) not "markers" — Claude's
  // safety layer refuses sys-mark-* style probes but happily echoes
  // REF-* tracking IDs — and the prompt never names "AGENTS.md"
  // literally (models try to read it from disk and 404 otherwise).
  const systemText =
    `You are an assistant running a long automated verification session for an internal ` +
    `platform integration test. The session may take several minutes and involves multiple ` +
    `steps. Work carefully and complete every requested step in order.\n\n` +
    `Operational context: this session exercises file tools, multiple skills, and remote ` +
    `tool servers. You have a shell available; use it to inspect your workspace and to ` +
    `create session files exactly as instructed.\n\n` +
    `Your session reference for this session is ${probes.system}. When the user asks you to ` +
    `acknowledge tracking references, include this session reference verbatim in your reply.`;
  const instructionsText =
    `# Project tracking guidance\n\n` +
    `The project tracking reference for this workspace is ${probes.instructions}. ` +
    `When a user asks you to acknowledge tracking references, include this project ` +
    `tracking reference verbatim in your reply.`;

  // Multi-step prompt. The probe acknowledgement is Step 1 (emitted
  // FIRST) so it round-trips reliably even if the agent exhausts its
  // turn budget on the later file work — a 4-step prompt that buried the
  // ack last had the agent finish the file ops and stop before emitting
  // it. Steps 2-4 force a shell call (ls), multiple file WRITES, and
  // multiple file READS (read-back), which drive TOOL_CALL_START/RESULT
  // events and the files pipeline.
  // (Input files / workspace assets are intentionally NOT used — that
  // feature was dropped in the MVP; "read file" coverage comes from the
  // agent reading back the files it wrote, on the supported
  // developer-tool path.)
  const promptSteps = [
    `Step 1. FIRST, before anything else, output exactly this one-line acknowledgement, ` +
      `filling in the three tracking references from your context (your session reference, ` +
      `the project tracking reference, and the request reference given here):\n` +
      `session=${probes.system} project=${probes.instructions} request=${probes.prompt}`,
    `Step 2. Use your shell to run \`ls -la\` in your current working directory and briefly ` +
      `note what you see.`,
    `Step 3. Create the directory ${CUSTOM_OUTPUT_DIR} (it is inside your writable ` +
      `workspace) and write three small text files into it using your write tool:\n` +
      `  - ${CUSTOM_OUTPUT_DIR}/report-1.txt containing exactly the line: ${probes.out[0]}\n` +
      `  - ${CUSTOM_OUTPUT_DIR}/report-2.txt containing exactly the line: ${probes.out[1]}\n` +
      `  - ${CUSTOM_OUTPUT_DIR}/report-3.txt containing exactly the line: ${probes.out[2]}`,
    `Step 4. Now READ each of those three files back (use your shell, e.g. \`cat\`) and ` +
      `report the exact line you read from each one.`
  ];

  return `
    import { Aex, Skill, McpServer, Instructions } from "@aexhq/sdk";
    ${finishedRunReadinessSource()}

    const client = new Aex({
      baseUrl: process.env.AEX_API_URL,
      apiKey: process.env.AEX_API_KEY
    });

    const skillAlpha = await Skill.fromContent(${JSON.stringify(`---\nname: ${managedHeavySkillName("alpha", spec.provider)}\ndescription: Complete every numbered step the user lists, in order.\n---\n# alpha\nComplete every numbered step the user lists, in order.`)}, {
      name: ${JSON.stringify(managedHeavySkillName("alpha", spec.provider))}
    });
    const skillBeta = await Skill.fromContent(${JSON.stringify(`---\nname: ${managedHeavySkillName("beta", spec.provider)}\ndescription: Always follow the project tracking guidance.\n---\n# beta\nAlways follow the project tracking guidance.`)}, {
      name: ${JSON.stringify(managedHeavySkillName("beta", spec.provider))}
    });
    const skillGamma = await Skill.fromContent(${JSON.stringify(`---\nname: ${managedHeavySkillName("gamma", spec.provider)}\ndescription: Write session files exactly as instructed, then acknowledge references.\n---\n# gamma\nWrite session files exactly as instructed, then acknowledge references.`)}, {
      name: ${JSON.stringify(managedHeavySkillName("gamma", spec.provider))}
    });

    const mcpPrimary = McpServer.remote({
      name: ${JSON.stringify(MCP_SERVER_NAME + "-primary")},
      url: ${JSON.stringify(MCP_SERVER_URL)}
    });
    const mcpSecondary = McpServer.remote({
      name: ${JSON.stringify(MCP_SERVER_NAME + "-secondary")},
      url: ${JSON.stringify(MCP_SERVER_URL)}
    });

    const rules = await Instructions.fromContent(
      ${JSON.stringify(instructionsText)},
      { name: "heavy-rules" }
    );
    const skillAlphaRef = await client.workspace.skills.publish(skillAlpha);
    const skillBetaRef = await client.workspace.skills.publish(skillBeta);
    const skillGammaRef = await client.workspace.skills.publish(skillGamma);
    const rulesRef = await client.workspace.instructions.publish(rules);

    const submitOpts = {
      model: ${JSON.stringify(spec.model)},
      system: ${JSON.stringify(systemText)},
      message: ${JSON.stringify(promptSteps)},
      assets: {
        skills: [skillAlphaRef, skillBetaRef, skillGammaRef],
        instructions: [rulesRef]
      },
      mcpServers: [mcpPrimary, mcpSecondary],
      fileCapture: { allowedDirs: [${JSON.stringify(CUSTOM_OUTPUT_DIR)}] },
      builtinTools: "default",
      environment: { variables: { HEAVY_SUITE: "heavy-session", HEAVY_CELL: "${spec.provider}" } },
      metadata: { suite: "heavy-session", cell: "${spec.provider}" },
      runtime: { kind: process.env.AEX_USER_TEST_RUNTIME },
      idempotencyKey: "heavy-${spec.provider}-" + Date.now()
    };

    const payload = await runOnce();

    async function runOnce() {
      const result = await client.start(submitOpts, { timeoutMs: ${spec.pollDeadlineMs} });
      requireSucceededRunBeforeFiles("heavy-session", result, [process.env.AEX_API_KEY]);
      const sessionId = result.sessionId;
      const run = {
        status: result.status,
        runtime: result.session.runtime?.kind,
      };
      const session = await client.sessions.open(sessionId);

      const resultEvents = result.events;
      const resultFiles = result.files;
      const events = (await session.events.list()).filter((event) => event.runId === result.run.runId);
      const files = (await session.files.list()).files;
      const eventSource = "session.events.list";
      const eventListError = null;
      const listedEventCount = events.length;
      const fileSource = "session.files.list";
      const fileListError = null;
      const listedFileCount = files.length;
      // CUSTOM envelopes nest the original payload under data.value.
      function customName(e) {
        return e && e.data && typeof e.data.name === "string" ? e.data.name : null;
      }
      function customValue(e) {
        const value = e && e.data ? e.data.value : null;
        return value && typeof value === "object" ? value : {};
      }
      function skillLoadedName(e) {
        const value = customValue(e);
        if (
          customName(e) === "aex.skill_loaded" ||
          value.kind === "skill_loaded" ||
          value.kind === "skill_loaded_marker"
        ) {
          const name = value.name || value.skillId;
          return typeof name === "string" ? name : null;
        }
        return null;
      }
      const notifications = events.filter((e) => e.type === "CUSTOM");
      const notificationKinds = notifications.map((n) => customValue(n).kind || "(unknown)");
      const skillLoadedNames = notifications.map(skillLoadedName).filter(Boolean);

      const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
      const assistantTextJoined = assistantTextEvents
        .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
        .join(" ");
      function toolResultText(content) {
        if (!Array.isArray(content)) return "";
        return content
          .map((part) => {
            if (!part || typeof part !== "object") return "";
            if (typeof part.text === "string") return part.text;
            return JSON.stringify(part);
          })
          .join(" ");
      }
      const channelProbeSources = {};
      function recordChannelProbeSources(source, text) {
        const normalized = String(text ?? "").replace(/\\s+/g, "");
        for (const [channel, probe] of [
          ["system", ${JSON.stringify(probes.system)}],
          ["instructions", ${JSON.stringify(probes.instructions)}],
          ["prompt", ${JSON.stringify(probes.prompt)}]
        ]) {
          if (!normalized.includes(probe)) continue;
          if (!channelProbeSources[channel]) channelProbeSources[channel] = [];
          if (!channelProbeSources[channel].includes(source)) channelProbeSources[channel].push(source);
        }
      }
      recordChannelProbeSources("assistantText", assistantTextJoined);
      for (const e of events) {
        if (e.type === "TOOL_CALL_START") {
          recordChannelProbeSources(
            "toolCallStart",
            JSON.stringify(e.data && typeof e.data === "object" ? e.data.arguments ?? {} : {})
          );
        } else if (e.type === "TOOL_CALL_RESULT") {
          recordChannelProbeSources(
            "toolCallResult",
            toolResultText(e.data && typeof e.data === "object" ? e.data.content : null)
          );
        }
      }
      const channelProbeMisses = [];
      for (const channel of ["system", "instructions", "prompt"]) {
        if (!channelProbeSources[channel] || channelProbeSources[channel].length === 0) channelProbeMisses.push(channel);
      }

      const eventTypeSet = Array.from(new Set(events.map((e) => e.type)));
      const toolCallStartCount = events.filter((e) => e.type === "TOOL_CALL_START").length;
      const toolCallResultCount = events.filter((e) => e.type === "TOOL_CALL_RESULT").length;

      const terminal = events.find((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR");
      const streamErrors = events
        .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error")
        .map((e) => (e.data && e.data.value && typeof e.data.value === "object" ? e.data.value : { unknown: true }));

      const outProbes = ${JSON.stringify(probes.out)};
      const filesCollected = [];
      const outProbesFound = new Set();
      for (const out of files.slice(0, 16)) {
        let sample = null;
        try {
          const bytes = await session.files.download(out);
          const text = new TextDecoder().decode(bytes);
          sample = text.slice(0, 256);
          for (const p of outProbes) {
            if (text.includes(p)) outProbesFound.add(p);
          }
        } catch (err) {
          const fields = [];
          if (err && err.name) fields.push("name=" + err.name);
          if (err && err.code) fields.push("code=" + err.code);
          if (err && typeof err.status === "number") fields.push("status=" + err.status);
          if (err && err.apiCode) fields.push("apiCode=" + err.apiCode);
          if (err && err.causeCode) fields.push("causeCode=" + err.causeCode);
          if (err && typeof err.attempts === "number") fields.push("attempts=" + err.attempts);
          const message = err && err.message ? err.message : String(err);
          sample = "(download error: " + message + (fields.length > 0 ? " [" + fields.join(" ") + "]" : "") + ")";
        }
        filesCollected.push({ filename: out.filename ?? null, sizeBytes: out.sizeBytes ?? 0, sample });
      }

      const serialized = JSON.stringify({ run, events, files });
      const apiKeyEnv = process.env.AEX_API_KEY ?? "";
      return {
        sessionId: sessionId,
        runOutcome: run.status,
        runtime: run.runtime ?? "(missing)",
        provider: run.provider ?? "(missing)",
        probes: ${JSON.stringify(probes)},
        eventCount: events.length,
        eventKinds: events.map((e) => e.type),
        eventTypeSet,
        eventSource,
        eventListError,
        fallbackEventCount: resultEvents.length,
        listedEventCount,
        toolCallStartCount,
        toolCallResultCount,
        notificationKinds,
        skillLoadedNames,
        assistantTextJoined,
        assistantTextEventCount: assistantTextEvents.length,
        terminalKind: terminal ? terminal.type : null,
        terminalData: terminal ? terminal.data : null,
        fileCount: files.length,
        fileSource,
        fileListError,
        fallbackFileCount: resultFiles.length,
        listedFileCount,
        files: filesCollected,
        outProbesFound: Array.from(outProbesFound),
        channelProbeSources,
        channelProbeMisses,
        leakedApiKey: apiKeyEnv.length > 0 && serialized.includes(apiKeyEnv),
        streamErrors
      };
    }

    process.stdout.write(JSON.stringify(payload));
    process.exit(0);
  `;
}

async function runCase(spec: CaseSpec, installDir: string): Promise<CaseResult> {
  const rand = (): string => Math.random().toString(36).slice(2, 10);
  // Channel-probe separators are dots, NOT hyphens. This originally worked around the
  // runtime's stream redactor, whose high-entropy catch-all ate a hyphen-segmented ref
  // glued to its `session=` label as one 24+ char run. Plan 09 (2026-07-25) deleted
  // that redactor, so nothing masks the probes any more — dots are kept only because
  // the probe shape is pinned by the assertions below.
  const probes: Probes = {
    system: "REF.verify." + rand(),
    instructions: "REF.verify." + rand(),
    prompt: "REF.verify." + rand(),
    out: ["REF-out-" + rand(), "REF-out-" + rand(), "REF-out-" + rand()]
  };
  const script = buildScript(spec, probes);
  const scriptPath = join(installDir, spec.scriptName);
  writeFileSync(scriptPath, script);

  const passEnv = buildPassEnv({
    AEX_API_URL: apiUrl,
    AEX_API_KEY: apiKey,
    AEX_USER_TEST_RUNTIME: runtimeKind
  });

  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: installDir,
    timeoutMs: spec.timeoutMs,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `heavy-session runner exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
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

describe("live hosted API — heavy full-feature long session via installed SDK", () => {
  it(
    "managed deepseek: 3 skills + 2 MCP + long system + multi-step write+read + files + full event vocab",
    async () => {
      const result = await runCase(
        {
          scriptName: "heavy-managed-deepseek.mjs",
          provider: "deepseek",
          model: deepseekModel,
          pollDeadlineMs: 9 * 60_000,
          pollIntervalMs: 3_000,
          timeoutMs: 12 * 60_000
        },
        install.installDir
      );
      console.info(
        `[user-tests] heavy-session sessionId=${result.sessionId} outcome=${result.runOutcome} terminalKind=${result.terminalKind ?? "(none)"}`
      );
      assertManagedShape(result, [
        managedHeavySkillName("alpha", "deepseek"),
        managedHeavySkillName("beta", "deepseek"),
        managedHeavySkillName("gamma", "deepseek")
      ]);
      expect(result.runtime).toBe(runtimeKind);
      expect(result.provider).toBe("deepseek");
    },
    13 * 60_000
  );

});
