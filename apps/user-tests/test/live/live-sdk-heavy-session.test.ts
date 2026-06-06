/**
 * Live scenario: live-sdk-heavy-session.test.ts
 *
 * The "heavy" full-feature long-session suite. Where
 * live-sdk-comprehensive.test.ts proves each cell works with a short
 * run, THIS suite submits one deliberately heavy, multi-minute session
 * per cell that exercises the *entire* customer feature surface at once
 * and validates EVERY observable aspect of the run. The goal is not to
 * test the model's capability — it is to prove the aex app
 * (materialization, BYOK proxy, event log, outputs pipeline, secret
 * redaction) behaves as expected under a maximal submission.
 *
 * Runs as an explicit gate AFTER the rest of the live user-tests pass
 * (own pnpm script `test:user:heavy` + own vitest config), so it is
 * never swept into the default `test:user` run. Wired into
 * live-user-tests.yml as the manual canary.
 *
 * Scope: one DeepSeek-managed cell. No other provider keys are provisioned in
 * CI for this public live canary.
 *
 * Each cell submits ONE run carrying the full surface together:
 *   - 3 inline Skills          multi-skill manifest + materialization
 *   - 2 remote MCP servers      multi-MCP recipe `extensions:` wiring
 *   - a long multi-paragraph `system` message (probe-tagged)
 *   - a multi-step `prompt` array (forces shell + multiple file WRITES +
 *                                  multiple file READS, then a probe ack)
 *   - 1 AGENTS.md               (probe-tagged project guidance)
 *   - a custom `outputs.allowedDirs` path (proves re-rooting under workspaceRoot)
 *   - `builtins: ["developer"]`, `environment.envVars`, `metadata`
 *   - `secrets` carrying the customer provider key
 *
 * Input files / workspace assets are intentionally NOT exercised — that
 * feature was dropped in the MVP for simplicity. "Read file" coverage
 * comes from the agent reading back the files it writes, on the
 * supported developer-tool path.
 *
 * Managed cells (full assertion set — "validate all aspects"):
 *   - run reached `succeeded`; runtime/provider echo back correctly.
 *   - event log is framed RUN_STARTED … RUN_FINISHED, terminal reason
 *     "complete", runtimeExitCode 0-or-absent.
 *   - FULL EVENT VOCABULARY: the distinct event types observed are a
 *     superset of every type aex emits on a successful run —
 *     RUN_STARTED, RUN_FINISHED, TEXT_MESSAGE_CONTENT, TOOL_CALL_START,
 *     TOOL_CALL_RESULT, CUSTOM (RUN_ERROR is failure-only and is covered
 *     by live-sdk-outputs-and-failures.test.ts). See AEX_EVENT_TYPES
 *     in packages/contracts/src/event-envelope.ts — the single source of
 *     truth this list is kept in sync with.
 *   - the agent actually used tools (TOOL_CALL_START/RESULT counts > 0).
 *   - all 3 submitted skills produced a `skill_loaded_marker` (every
 *     skill materialized into the container).
 *   - system + AGENTS.md + prompt probes all round-trip in the reply
 *     (composeInstructions carried all three channels into the recipe).
 *   - the OUTPUTS pipeline round-trips: every captured output downloads
 *     without error, and at least one agent-written file carries its
 *     expected REF-out token (write → capture → object storage → download e2e).
 *   - no secret value (provider key, runner bearer) appears anywhere in
 *     the SDK-visible payload (run + events + outputs).
 *
 * Required env:
 *   AEX_API_URL                live hosted API URL (local or prod)
 *   AEX_API_TOKEN               workspace API token
 *   DEEPSEEK_API_KEY                customer DeepSeek API key
 *   AEX_USER_TEST_TARBALL            packed SDK tarball
 *     OR AEX_USER_TEST_VERSION       published version on npm
 * Optional:
 *   AEX_USER_TEST_DEEPSEEK_MODEL    default "deepseek-chat"
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (heavy-session): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"] ?? "deepseek-chat";

// DeepWiki MCP — public, unauthenticated, exposes GitHub repo Q&A tools.
const MCP_SERVER_URL = "https://mcp.deepwiki.com/mcp";
const MCP_SERVER_NAME = "deepwiki";

// Custom explicit outputs path. Must be BOTH agent-writable and
// captured by outputs.allowedDirs: the runner's resolveInsideWorkspace re-roots
// allowed dirs under workspaceRoot (/workspace) and dedupes a leading
// `workspace/` segment, so "/workspace/outputs/heavy" captures exactly
// the path the agent writes to. (An arbitrary path like /data/... gets
// captured at <root>/data/... but the agent can't write to literal
// /data — its writable tree is /workspace.)
const CUSTOM_OUTPUT_DIR = "/workspace/outputs/heavy";

// Every event type aex emits on a SUCCESSFUL run. Mirror of
// AEX_EVENT_TYPES in packages/contracts/src/event-envelope.ts minus
// RUN_ERROR (failure-only; covered by live-sdk-outputs-and-failures).
// Hardcoded rather than imported because the user-tests layer never
// imports @aexhq/* workspace packages.
const EXPECTED_SUCCESS_EVENT_TYPES = [
  "RUN_STARTED",
  "RUN_FINISHED",
  "TEXT_MESSAGE_CONTENT",
  "TOOL_CALL_START",
  "TOOL_CALL_RESULT",
  "CUSTOM"
] as const;

interface Probes {
  readonly system: string;
  readonly agentsMd: string;
  readonly prompt: string;
  readonly out: readonly [string, string, string];
}

interface CaseResult {
  readonly runId: string;
  readonly runStatus: string;
  readonly runtime: string;
  readonly provider: string;
  readonly probes: Probes;
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly eventTypeSet: readonly string[];
  readonly toolCallStartCount: number;
  readonly toolCallResultCount: number;
  readonly notificationKinds: readonly string[];
  readonly skillLoadedNames: readonly string[];
  readonly assistantTextJoined: string;
  readonly assistantTextEventCount: number;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly outputCount: number;
  readonly outputs: readonly { filename: string | null; sizeBytes: number; sample: string | null }[];
  readonly outProbesFound: readonly string[];
  readonly leakedProviderKey: boolean;
  readonly leakedDeepseekKey: boolean;
  // Full payload of every runner-sourced stream_error — captures the
  // actual exception message + phase when materialize / manifest fetch
  // fails, right before a runner_error terminal.
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

interface CaseSpec {
  readonly scriptName: string;
  readonly runtime: "managed";
  readonly provider: "deepseek";
  readonly model: string;
  readonly keyEnvName: string;
  readonly keyValue: string;
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
    `Operational context: this run exercises file tools, multiple skills, and remote ` +
    `tool servers. You have a shell available; use it to inspect your workspace and to ` +
    `create output files exactly as instructed.\n\n` +
    `Your session reference for this run is ${probes.system}. When the user asks you to ` +
    `acknowledge tracking references, include this session reference verbatim in your reply.`;
  const agentsMdText =
    `# Project tracking guidance\n\n` +
    `The project tracking reference for this workspace is ${probes.agentsMd}. ` +
    `When a user asks you to acknowledge tracking references, include this project ` +
    `tracking reference verbatim in your reply.`;

  // Multi-step prompt. The probe acknowledgement is Step 1 (emitted
  // FIRST) so it round-trips reliably even if the agent exhausts its
  // turn budget on the later file work — a 4-step prompt that buried the
  // ack last had the agent finish the file ops and stop before emitting
  // it. Steps 2-4 force a shell call (ls), multiple file WRITES, and
  // multiple file READS (read-back), which drive TOOL_CALL_START/RESULT
  // events and the outputs pipeline.
  // (Input files / workspace assets are intentionally NOT used — that
  // feature was dropped in the MVP; "read file" coverage comes from the
  // agent reading back the files it wrote, on the supported
  // developer-tool path.)
  const promptSteps = [
    `Step 1. FIRST, before anything else, output exactly this one-line acknowledgement, ` +
      `filling in the three tracking references from your context (your session reference, ` +
      `the project tracking reference, and the request reference given here):\n` +
      `session=${probes.system} project=${probes.agentsMd} request=${probes.prompt}`,
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
    import { AgentExecutor, Skill, McpServer, AgentsMd } from "@aexhq/sdk";

    const client = new AgentExecutor({
      baseUrl: process.env.AEX_API_URL,
      apiToken: process.env.AEX_API_TOKEN
    });

    // SKILL.md starts with YAML frontmatter so each test skill is
    // self-describing and produces a stable skill_loaded_marker name.
    const skillAlpha = await Skill.fromFiles({
      name: "heavy-alpha-${spec.runtime}",
      files: { "SKILL.md": "---\\nname: heavy-alpha-${spec.runtime}\\ndescription: Complete every numbered step the user lists, in order.\\n---\\n# alpha\\nComplete every numbered step the user lists, in order." }
    });
    const skillBeta = await Skill.fromFiles({
      name: "heavy-beta-${spec.runtime}",
      files: { "SKILL.md": "---\\nname: heavy-beta-${spec.runtime}\\ndescription: Always follow the project tracking guidance.\\n---\\n# beta\\nAlways follow the project tracking guidance." }
    });
    const skillGamma = await Skill.fromFiles({
      name: "heavy-gamma-${spec.runtime}",
      files: { "SKILL.md": "---\\nname: heavy-gamma-${spec.runtime}\\ndescription: Write output files exactly as instructed, then acknowledge references.\\n---\\n# gamma\\nWrite output files exactly as instructed, then acknowledge references." }
    });

    const mcpPrimary = McpServer.remote({
      name: ${JSON.stringify(MCP_SERVER_NAME + "-primary")},
      url: ${JSON.stringify(MCP_SERVER_URL)}
    });
    const mcpSecondary = McpServer.remote({
      name: ${JSON.stringify(MCP_SERVER_NAME + "-secondary")},
      url: ${JSON.stringify(MCP_SERVER_URL)}
    });

    const rules = await AgentsMd.fromContent(
      ${JSON.stringify(agentsMdText)},
      { name: "heavy-rules" }
    );

    const submitOpts = {
      provider: ${JSON.stringify(spec.provider)},
      runtime: ${JSON.stringify(spec.runtime)},
      model: ${JSON.stringify(spec.model)},
      system: ${JSON.stringify(systemText)},
      prompt: ${JSON.stringify(promptSteps)},
      skills: [skillAlpha, skillBeta, skillGamma],
      mcpServers: [mcpPrimary, mcpSecondary],
      agentsMd: [rules],
      outputs: { allowedDirs: [${JSON.stringify(CUSTOM_OUTPUT_DIR)}] },
      builtins: ["developer"],
      environment: { envVars: { HEAVY_SUITE: "heavy-session", HEAVY_CELL: "${spec.runtime}-${spec.provider}" } },
      metadata: { suite: "heavy-session", cell: "${spec.runtime}-${spec.provider}" },
      secrets: { ${spec.provider}: { apiKey: process.env.${spec.keyEnvName} } },
      idempotencyKey: "heavy-${spec.runtime}-${spec.provider}-" + Date.now()
    };

    const runId = await client.submitRun(submitOpts);

    // Block on the SDK's own terminal-wait instead of a
    // hand-rolled poll: it covers every terminal status — including
    // \`timed_out\`, which the old succeeded/failed/cancelled break-set
    // silently polled through — and throws on the deadline.
    let run;
    try {
      run = await client.wait(runId, { timeoutMs: ${spec.pollDeadlineMs}, intervalMs: ${spec.pollIntervalMs} });
    } catch (err) {
      const last = await client.getRun(runId).catch(() => null);
      process.stderr.write(JSON.stringify({ kind: "timeout", error: err && err.message, run: last }, null, 2));
      process.exit(2);
    }

    const events = await client.listEvents(runId);
    const outputs = await client.listOutputs(runId);

    // CUSTOM envelopes nest the original payload under data.value.
    const notifications = events.filter((e) => e.type === "CUSTOM");
    const notificationKinds = notifications.map((n) => (n.data && n.data.value && n.data.value.kind) || "(unknown)");
    const skillLoaded = notifications.filter((n) => (n.data && n.data.value && n.data.value.kind) === "skill_loaded_marker");
    const skillLoadedNames = skillLoaded.map((n) => (n.data && n.data.value && n.data.value.name) || null).filter(Boolean);

    const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
    const assistantTextJoined = assistantTextEvents
      .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
      .join(" ");

    const eventTypeSet = Array.from(new Set(events.map((e) => e.type)));
    const toolCallStartCount = events.filter((e) => e.type === "TOOL_CALL_START").length;
    const toolCallResultCount = events.filter((e) => e.type === "TOOL_CALL_RESULT").length;

    const terminal = events.find((e) => (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR"));
    const streamErrors = events
      .filter((e) => e.type === "CUSTOM" && e.data && e.data.value && e.data.value.source === "runner")
      .map((e) => (e.data && typeof e.data === "object" ? e.data : { unknown: true }));

    const outProbes = ${JSON.stringify(probes.out)};
    const outputsCollected = [];
    const outProbesFound = new Set();
    for (const out of outputs.slice(0, 16)) {
      let sample = null;
      try {
        const bytes = await client.downloadOutput(runId, out);
        const text = new TextDecoder().decode(bytes);
        sample = text.slice(0, 256);
        for (const p of outProbes) {
          if (text.includes(p)) outProbesFound.add(p);
        }
      } catch (err) {
        sample = "(download error: " + (err && err.message ? err.message : String(err)) + ")";
      }
      outputsCollected.push({ filename: out.filename ?? null, sizeBytes: out.sizeBytes ?? 0, sample });
    }

    const serialized = JSON.stringify({ run, events, outputs });
    const deepseekEnv = process.env.DEEPSEEK_KEY ?? "";
    const result = {
      runId: runId,
      runStatus: run.status,
      runtime: run.runtime ?? "(missing)",
      provider: run.provider ?? "(missing)",
      probes: ${JSON.stringify(probes)},
      eventCount: events.length,
      eventKinds: events.map((e) => e.type),
      eventTypeSet,
      toolCallStartCount,
      toolCallResultCount,
      notificationKinds,
      skillLoadedNames,
      assistantTextJoined,
      assistantTextEventCount: assistantTextEvents.length,
      terminalKind: terminal ? terminal.type : null,
      terminalData: terminal ? terminal.data : null,
      outputCount: outputs.length,
      outputs: outputsCollected,
      outProbesFound: Array.from(outProbesFound),
      leakedProviderKey: deepseekEnv.length > 0 && serialized.includes(deepseekEnv),
      leakedDeepseekKey: deepseekEnv.length > 0 && serialized.includes(deepseekEnv),
      streamErrors
    };
    process.stdout.write(JSON.stringify(result));
    process.exit(0);
  `;
}

async function runCase(spec: CaseSpec, installDir: string): Promise<CaseResult> {
  const rand = (): string => Math.random().toString(36).slice(2, 10);
  // Channel-probe separators are dots, NOT hyphens. The stream-before-disk
  // runtime redactor masks high-entropy
  // runs of [A-Za-z0-9+/=-]{24,}. The model echoes these probes in a
  // key=value shape ("session=<ref> project=<ref> request=<ref>"), and a
  // hyphen-segmented ref glued to its `session=` label forms one 24+ char
  // run that the redactor eats whole — the probe never survives into the
  // managed-runtime stdout the event stream is built from. A dot is OUTSIDE that
  // char class, so it splits the run into sub-24-char segments that
  // survive regardless of how the model punctuates the reply. REF-out tokens
  // go to output FILES, not the redacted stdout stream, so they keep hyphens.
  const probes: Probes = {
    system: "REF.verify." + rand(),
    agentsMd: "REF.verify." + rand(),
    prompt: "REF.verify." + rand(),
    out: ["REF-out-" + rand(), "REF-out-" + rand(), "REF-out-" + rand()]
  };
  const script = buildScript(spec, probes);
  const scriptPath = join(installDir, spec.scriptName);
  writeFileSync(scriptPath, script);

  const passEnv = buildPassEnv({
    AEX_API_URL: apiUrl,
    AEX_API_TOKEN: apiToken,
    [spec.keyEnvName]: spec.keyValue,
    DEEPSEEK_KEY: deepseekKey
  });

  const child = await runCommand(process.execPath, [scriptPath], {
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

// Self-diagnosing dump shared by every assertion path. The heavy case
// touches many surfaces, so a failure must be self-explanatory from the
// CI log alone (.managed-runtime-logs samples + streamErrors carry the real cause).
function dumpCase(result: CaseResult): string {
  const lines: string[] = [];
  lines.push(`runId=${result.runId} runtime=${result.runtime} provider=${result.provider}`);
  lines.push(`runStatus=${result.runStatus} terminalKind=${result.terminalKind}`);
  lines.push(`terminalData=${JSON.stringify(result.terminalData)}`);
  lines.push(`eventTypeSet=[${result.eventTypeSet.join(", ")}]`);
  lines.push(
    `toolCallStart=${result.toolCallStartCount} toolCallResult=${result.toolCallResultCount} ` +
      `assistantText=${result.assistantTextEventCount} events=${result.eventCount}`
  );
  lines.push(`notificationKinds=[${result.notificationKinds.join(", ")}]`);
  lines.push(`skillLoadedNames=[${result.skillLoadedNames.join(", ")}]`);
  lines.push(`outProbesFound=[${result.outProbesFound.join(", ")}] of [${result.probes.out.join(", ")}]`);
  if (result.streamErrors.length > 0) {
    lines.push(`streamErrors:`);
    for (const se of result.streamErrors) {
      lines.push(`  - ${JSON.stringify(se).slice(0, 800)}`);
    }
  }
  lines.push(`outputs=${result.outputs.map((o) => `${o.filename}(${o.sizeBytes}B)`).join(", ")}`);
  for (const o of result.outputs) {
    if (o.filename && o.filename.startsWith(".runtime/")) {
      lines.push(`--- ${o.filename} (sample, first 256 bytes) ---`);
      lines.push(o.sample ?? "(empty)");
      lines.push(`--- end ${o.filename} ---`);
    }
  }
  lines.push(`assistantTextJoined=${result.assistantTextJoined.slice(0, 1000)}`);
  return lines.join("\n");
}

function assertManagedShape(result: CaseResult, expectedSkillPrefixes: readonly [string, string, string]): void {
  const dump = (): string => dumpCase(result);

  // Lifecycle: succeeded, framed RUN_STARTED … RUN_FINISHED, terminal
  // reason "complete", runtimeExitCode 0-or-absent.
  if (result.runStatus !== "succeeded") {
    throw new Error(`expected runStatus "succeeded" but got "${result.runStatus}"\n\n${dump()}`);
  }
  expect(result.eventKinds[0]).toBe("RUN_STARTED");
  expect(result.eventKinds[result.eventKinds.length - 1]).toBe("RUN_FINISHED");
  expect(result.terminalKind).toBe("RUN_FINISHED");
  const terminal = result.terminalData ?? {};
  if (terminal["reason"] !== "complete") {
    throw new Error(`expected terminal reason "complete" but got "${terminal["reason"]}"\n\n${dump()}`);
  }
  const exitCode = terminal["runtimeExitCode"];
  if (exitCode !== undefined && exitCode !== 0) {
    throw new Error(`runtimeExitCode=${exitCode}\n\n${dump()}`);
  }

  // Full event vocabulary: every type aex emits on success is
  // present. Forced by the submission (text reply, ls + write tool
  // calls, skill_loaded_marker CUSTOM notifications).
  for (const type of EXPECTED_SUCCESS_EVENT_TYPES) {
    if (!result.eventTypeSet.includes(type)) {
      throw new Error(`expected event type "${type}" was not observed\n\n${dump()}`);
    }
  }
  // The agent actually used tools.
  if (result.toolCallStartCount <= 0 || result.toolCallResultCount <= 0) {
    throw new Error(
      `expected tool-call events (start>0 && result>0) but got start=${result.toolCallStartCount} result=${result.toolCallResultCount}\n\n${dump()}`
    );
  }

  // Every submitted skill materialized into the container.
  expect(result.skillLoadedNames.length, dump()).toBeGreaterThanOrEqual(3);
  for (const prefix of expectedSkillPrefixes) {
    if (!result.skillLoadedNames.some((n) => n.startsWith(prefix))) {
      throw new Error(`skill "${prefix}" produced no skill_loaded_marker\n\n${dump()}`);
    }
  }

  // The agent produced a real reply.
  expect(result.assistantTextEventCount).toBeGreaterThan(0);
  expect(result.assistantTextJoined.length).toBeGreaterThan(0);

  // system + AGENTS.md + prompt all reached the model via the recipe.
  // Strip whitespace because stream-json fragments content per token.
  const normalized = result.assistantTextJoined.replace(/\s+/g, "");
  for (const [channel, probe] of [
    ["system", result.probes.system],
    ["agentsMd", result.probes.agentsMd],
    ["prompt", result.probes.prompt]
  ] as const) {
    if (!normalized.includes(probe)) {
      throw new Error(`${channel} probe "${probe}" did not round-trip in the reply\n\n${dump()}`);
    }
  }

  // Outputs pipeline: every captured output downloads cleanly, and at
  // least one agent-written file carries its REF-out token (the
  // write → capture → object storage → download path works end-to-end). We require
  // ≥1 (not all 3) so a single missed write doesn't flip a hard gate on
  // model write-variance — the dump records exactly which were found.
  for (const out of result.outputs) {
    expect(out.sizeBytes).toBeGreaterThanOrEqual(0);
    if (out.sample !== null) {
      expect(out.sample.startsWith("(download error"), dump()).toBe(false);
    }
  }
  if (result.outProbesFound.length < 1) {
    throw new Error(
      `no agent-written output file carried any expected REF-out token; outputs pipeline unverified\n\n${dump()}`
    );
  }

  // No secret leakage anywhere in the SDK-visible payload.
  expect(result.leakedProviderKey, dump()).toBe(false);
  expect(result.leakedDeepseekKey, dump()).toBe(false);
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
    "managed deepseek: 3 skills + 2 MCP + long system + multi-step write+read + outputs + full event vocab",
    async () => {
      const result = await runCase(
        {
          scriptName: "heavy-managed-deepseek.mjs",
          runtime: "managed",
          provider: "deepseek",
          model: deepseekModel,
          keyEnvName: "DEEPSEEK_KEY_SUBMIT",
          keyValue: deepseekKey,
          pollDeadlineMs: 9 * 60_000,
          pollIntervalMs: 3_000,
          timeoutMs: 12 * 60_000
        },
        install.installDir
      );
      assertManagedShape(result, ["heavy-alpha-managed", "heavy-beta-managed", "heavy-gamma-managed"]);
      expect(result.runtime).toBe("managed");
      expect(result.provider).toBe("deepseek");
    },
    13 * 60_000
  );

});
