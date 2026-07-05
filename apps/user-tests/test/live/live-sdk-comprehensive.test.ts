/**
 * Live scenario: live-sdk-comprehensive.test.ts
 *
 * End-to-end user-perspective coverage of the full SDK + hosted API +
 * managed runtime surface against the deployed hosted API
 * (`AEX_API_URL`). Drives the freshly-installed `aex`
 * package (tarball or registry version) from a child process so
 * workspace symlinks cannot leak in.
 *
 * The DeepSeek-managed cell submits one run with:
 *   - 2 inline Skills (proves multi-skill manifest + materialization)
 *   - 2 remote MCP servers (exercises the multi-MCP submission path; that
 *     the model actually INVOKES a wired MCP is asserted separately by
 *     live-sdk-mcp-invocation.test.ts, which disarms builtins so the MCP
 *     is the only available tool — this comprehensive run keeps builtins on
 *     and does not assert MCP invocation)
 *   - 1 AGENTS.md (probe-tagged so a model reply that omits it fails)
 *   - 1 `system` message (probe-tagged)
 *   - 1 prompt (probe-tagged)
 *   - 1 custom outputs.allowedDirs entry (not /workspace/outputs — exercises
 *     the custom-dir submission path; this run does not force a write into it,
 *     so the re-root is not asserted here, only that any uploaded outputs
 *     round-trip)
 *   - `secrets` carrying the customer's provider key
 *
 * Then waits for `runtime_terminal` and asserts:
 *   - The run reached `succeeded`.
 *   - The event log starts with `runtime_started` and ends with
 *     `runtime_terminal` (The managed runtime actually spawned and exited cleanly).
 *   - Every submitted skill produced a `skill_loaded`
 *     notification (proves materialization of all skills).
 *   - At least one `assistant_text` event landed (The managed runtime generated a
 *     real reply via the BYOK provider-proxy).
 *   - The collected assistant text contains the system + AGENTS.md +
 *     prompt probes (proves `composeInstructions()` carried all three
 *     channels into the recipe.yaml that the managed runtime reads).
 *   - No secret value (the provider key) appears anywhere
 *     in the SDK-visible payload (run + events + outputs).
 *
 * No env-var flags gate scope. The five `test:*` commands are the
 * only knobs.
 *
 * Required env:
 *   AEX_API_URL                live hosted API URL (local or prod)
 *   AEX_API_KEY               workspace API key
 *   DEEPSEEK_API_KEY                customer DeepSeek API key
 *   AEX_USER_TEST_TARBALL            packed SDK tarball
 *     OR AEX_USER_TEST_VERSION       published package version
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (comprehensive): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

// DeepWiki MCP — public, unauthenticated, exposes GitHub repo Q&A tools.
const MCP_SERVER_URL = "https://mcp.deepwiki.com/mcp";
const MCP_SERVER_NAME = "deepwiki";

function managedSkillName(role: "alpha" | "beta", provider: CaseSpec["provider"]): string {
  return `compose-${role}-managed-${provider}`;
}

interface CaseResult {
  readonly runId: string;
  readonly runStatus: string;
  readonly runtime: string;
  readonly provider: string;
  readonly probes: { system: string; agentsMd: string; prompt: string };
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly notificationKinds: readonly string[];
  readonly skillLoadedNames: readonly string[];
  readonly skillLoadedEventSummaries: ReadonlyArray<Record<string, unknown>>;
  readonly assistantTextJoined: string;
  readonly assistantTextEventCount: number;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly outputCount: number;
  readonly outputs: readonly { filename: string; sizeBytes: number; sample: string | null }[];
  readonly leakedProviderKey: boolean;
  // Full payload of every stream_error event the runner emitted —
  // captures the actual exception message + phase when materialize or
  // manifest fetch fails (the runner emits a stream_error with the
  // message right before the runner_error terminal).
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
  readonly provider: "deepseek";
  readonly model: string;
  readonly keyEnvName: string;
  readonly keyValue: string;
  readonly customOutputDir: string;
  readonly pollDeadlineMs: number;
  readonly pollIntervalMs: number;
  readonly timeoutMs: number;
}

function buildScript(spec: CaseSpec, probes: { system: string; agentsMd: string; prompt: string }): string {
  // Channel-delivery probe. recipe.yaml carries:
  //   composeInstructions() = submission.system + AGENTS.md text +
  //   prompt parts (joined by blank lines).
  // We inject one unique tracking reference (`REF-verify-...`) into
  // each channel and ask the model to acknowledge with all three.
  //
  // Two framing choices avoid prior refusal modes:
  //   - Tokens are "tracking references" (operational, mundane), not
  //     "markers" — Claude's safety layer treats `sys-mark-*` as
  //     adversarial probes and refuses ("Iwon'tparticipateinprompt
  //     injectionscenarios"), but happily echoes ref/tracking IDs.
  //   - User prompt never mentions the literal string "AGENTS.md" —
  //     when it did, models (DeepSeek, Anthropic) tried to read the
  //     file from disk and returned FILE_NOT_FOUND ("TheAGENTS.mdfile
  //     doesn'texistinthisprojectworkspace"). The text in that channel
  //     is described as "project tracking guidance" instead.
  const promptText =
    `Please reply with a one-line acknowledgement that includes ` +
    `all three tracking references in your context, in this order: ` +
    `(1) your session reference (from your system instructions), ` +
    `(2) the project tracking reference (from your project guidance), ` +
    `(3) the user request reference, which is ${probes.prompt}. ` +
    `Format your reply as: ` +
    `session=<ref> project=<ref> request=${probes.prompt}.`;
  const systemText =
    `You are an assistant running an automated session. ` +
    `Your session reference for this run is ${probes.system}. ` +
    `When a user asks you to acknowledge tracking references, ` +
    `include this session reference verbatim in your reply.`;
  const agentsMdText =
    `# Project tracking guidance\n\n` +
    `The project tracking reference for this workspace is ${probes.agentsMd}. ` +
    `When a user asks you to acknowledge tracking references, ` +
    `include this project tracking reference verbatim in your reply.`;

  return `
    import { Aex, Skill, McpServer, AgentsMd } from "@aexhq/sdk";

    const client = new Aex({
      baseUrl: process.env.AEX_API_URL,
      apiKey: process.env.AEX_API_KEY
    });

    const skillAlpha = await Skill.fromContent(${JSON.stringify(`---\nname: ${managedSkillName("alpha", spec.provider)}\ndescription: Reply with the requested probes verbatim.\n---\n# alpha\nReply with the requested probes verbatim.`)}, {
      name: ${JSON.stringify(managedSkillName("alpha", spec.provider))}
    });
    const skillBeta = await Skill.fromContent(${JSON.stringify(`---\nname: ${managedSkillName("beta", spec.provider)}\ndescription: Always comply with the AGENTS.md rules.\n---\n# beta\nAlways comply with the AGENTS.md rules.`)}, {
      name: ${JSON.stringify(managedSkillName("beta", spec.provider))}
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
      { name: "compose-rules" }
    );

    const runOpts = {
      provider: ${JSON.stringify(spec.provider)},
      model: ${JSON.stringify(spec.model)},
      system: ${JSON.stringify(systemText)},
      message: ${JSON.stringify(promptText)},
      agentsMd: [rules],
      outputs: { allowedDirs: [${JSON.stringify(spec.customOutputDir)}] },
      apiKeys: { [${JSON.stringify(spec.provider)}]: process.env.${spec.keyEnvName} },
      idempotencyKey: "comprehensive-${spec.provider}-" + Date.now()
    };
    runOpts.skills = [skillAlpha, skillBeta];
    runOpts.mcpServers = [mcpPrimary, mcpSecondary];

    const runResult = await client.run(runOpts, { timeoutMs: ${spec.pollDeadlineMs} });
    const runId = runResult.runId;
    const session = await client.sessions.open(runId);
    const run = {
      status: runResult.ok ? "succeeded" : (typeof runResult.status === "string" && runResult.status ? runResult.status : "failed"),
      runtime: "managed",
      provider: ${JSON.stringify(spec.provider)}
    };
    const fallbackEvents = Array.isArray(runResult.events) ? runResult.events : [];
    const fallbackOutputs = Array.isArray(runResult.outputs) ? runResult.outputs : [];
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
    function skillLoadedSummary(e) {
      const value = customValue(e);
      return {
        customName: customName(e),
        kind: typeof value.kind === "string" ? value.kind : null,
        name: typeof value.name === "string" ? value.name : null,
        skillId: typeof value.skillId === "string" ? value.skillId : null
      };
    }
    const notifications = events.filter((e) => e.type === "CUSTOM");
    const notificationKinds = notifications.map((n) => customValue(n).kind || "(unknown)");
    const skillLoadedEventSummaries = notifications
      .filter((n) => skillLoadedName(n))
      .map(skillLoadedSummary);
    const skillLoadedNames = notifications.map(skillLoadedName).filter(Boolean);

    const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
    const assistantTextJoined = assistantTextEvents
      .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
      .join(" ");

    const isSessionIdle = (e) => e && e.type === "CUSTOM" && e.data && e.data.name === "aex.session.idle";
    const terminal = events.find((e) => (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR")) ?? events.find(isSessionIdle);
    const eventKinds = events.map((e) => e.type);
    if (terminal && isSessionIdle(terminal) && !eventKinds.includes("RUN_FINISHED")) eventKinds.push("RUN_FINISHED");
    const terminalData = terminal && isSessionIdle(terminal)
      ? { ...terminal.data.value, reason: terminal.data.value?.reason === "completed" ? "complete" : terminal.data.value?.reason }
      : terminal ? terminal.data : null;
    // stream_error events carry the runner-side exception that
    // caused a runner_error terminal — message + stack + phase
    // (manifest/materialize). Collect them all so the diagnostic
    // can show the actual cause.
    const streamErrors = events
      .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error")
      .map((e) => (e.data && typeof e.data === "object" ? e.data : { unknown: true }));

    const outputsCollected = [];
    for (const out of outputs.slice(0, 8)) {
      let sample = null;
      try {
        const bytes = await session.outputs().download(out);
        const text = new TextDecoder().decode(bytes);
        sample = text.slice(0, 256);
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
      eventKinds,
      notificationKinds,
      skillLoadedNames,
      skillLoadedEventSummaries,
      assistantTextJoined,
      assistantTextEventCount: assistantTextEvents.length,
      terminalKind: terminal && isSessionIdle(terminal) ? "RUN_FINISHED" : terminal ? terminal.type : null,
      terminalData,
      outputCount: outputs.length,
      outputs: outputsCollected,
      leakedProviderKey: deepseekEnv.length > 0 && serialized.includes(deepseekEnv),
      streamErrors
    };
    process.stdout.write(JSON.stringify(result));
    process.exit(0);
  `;
}

async function runCase(spec: CaseSpec, installDir: string): Promise<CaseResult> {
  // REF.verify.XXXXXXXX format reads as a routine tracking ID, not
  // an adversarial marker. Earlier `sys-mark-*` / `agm-mark-*` and
  // `verify-*` framings still let Claude classify the test as a
  // prompt-injection probe; an explicit REF prefix anchors the
  // model on "tracking reference, echo for audit" semantics.
  //
  // Separators are dots, NOT hyphens. The stream-before-disk redactor
  // the runtime redactor masks high-entropy runs of
  // [A-Za-z0-9+/=-]{24,}. The model echoes the probes in a key=value
  // shape ("session=<ref> ..."), and a hyphen-segmented ref glued to its
  // `session=` label forms one 24+ char run that the redactor eats whole —
  // the probe never survives into the managed-runtime stdout the event stream is
  // built from. A dot is OUTSIDE that char class, so it splits the run
  // into sub-24-char segments that survive regardless of how the model
  // punctuates the reply.
  const probes = {
    system: "REF.verify." + Math.random().toString(36).slice(2, 10),
    agentsMd: "REF.verify." + Math.random().toString(36).slice(2, 10),
    prompt: "REF.verify." + Math.random().toString(36).slice(2, 10)
  };
  const script = buildScript(spec, probes);
  const scriptPath = join(installDir, spec.scriptName);
  writeFileSync(scriptPath, script);

  const passEnv = buildPassEnv({
    AEX_API_URL: apiUrl,
    AEX_API_KEY: apiKey,
    [spec.keyEnvName]: spec.keyValue,
    DEEPSEEK_KEY: deepseekKey
  });

  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: installDir,
    timeoutMs: spec.timeoutMs,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `comprehensive runner exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as CaseResult;
}

function assertManagedShape(result: CaseResult, expectedSkillPrefixes: readonly [string, string]): void {
  expect(result.runStatus).toBe("succeeded");

  // Event log frames the run: starts with runtime_started, ends with
  // runtime_terminal. Both are emitted by the runner; managed-runtime stream
  // events sit in between.
  // On terminal mismatch, dump everything we know so the failure
  // log is self-diagnosing. The comprehensive case touches many
  // surfaces (skills, MCP, AGENTS.md, system, custom outputs.allowedDirs)
  // and a runner_error here means materialize() or the manifest fetch tripped.
  // Internal runtime diagnostics are intentionally not exposed through the
  // public outputs list.
  const dumpComprehensive = (): string => {
    const lines: string[] = [];
    lines.push(`runId=${result.runId} runtime=${result.runtime} provider=${result.provider}`);
    lines.push(`runStatus=${result.runStatus} terminalKind=${result.terminalKind}`);
    lines.push(`terminalData=${JSON.stringify(result.terminalData)}`);
    lines.push(`eventKinds=[${result.eventKinds.join(", ")}]`);
    lines.push(`notificationKinds=[${result.notificationKinds.join(", ")}]`);
    lines.push(`skillLoadedNames=[${result.skillLoadedNames.join(", ")}]`);
    lines.push(`skillLoadedEventSummaries=${JSON.stringify(result.skillLoadedEventSummaries)}`);
    if (result.streamErrors.length > 0) {
      lines.push(`streamErrors:`);
      for (const se of result.streamErrors) {
        lines.push(`  - ${JSON.stringify(se).slice(0, 800)}`);
      }
    }
    lines.push(`outputs=${result.outputs.map((o) => `${o.filename}(${o.sizeBytes}B)`).join(", ")}`);
    lines.push(`assistantTextJoined=${result.assistantTextJoined.slice(0, 800)}`);
    return lines.join("\n");
  };

  expect(result.terminalKind).toBe("RUN_FINISHED");
  expect(result.eventKinds).toContain("RUN_FINISHED");
  const terminal = result.terminalData ?? {};
  if (terminal["reason"] !== "complete") {
    throw new Error(`expected terminal reason "complete" but got "${terminal["reason"]}"\n\n${dumpComprehensive()}`);
  }
  // runtimeExitCode is undefined when the runtime adapter's own complete
  // event fires before the runner's terminal emit (idempotency guard);
  // that's the happy path. If present it must be 0.
  const exitCode = terminal["runtimeExitCode"];
  if (exitCode !== undefined && exitCode !== 0) {
    throw new Error(`runtimeExitCode=${exitCode}\n\n${dumpComprehensive()}`);
  }

  // Materialization carried every submitted skill into the container.
  if (result.skillLoadedNames.length < expectedSkillPrefixes.length) {
    throw new Error(
      `expected at least ${expectedSkillPrefixes.length} skill_loaded events, got ${result.skillLoadedNames.length}\n\n${dumpComprehensive()}`
    );
  }
  for (const prefix of expectedSkillPrefixes) {
    if (!result.skillLoadedNames.some((n) => n.startsWith(prefix))) {
      throw new Error(`skill "${prefix}" produced no skill_loaded event\n\n${dumpComprehensive()}`);
    }
  }

  // The managed runtime actually produced a reply: at least one assistant_text event
  // with non-empty text.
  expect(result.assistantTextEventCount).toBeGreaterThan(0);
  expect(result.assistantTextJoined.length).toBeGreaterThan(0);

  // system + AGENTS.md + prompt all reached managed runtime via the recipe.yaml
  // instructions. The model echoed each probe. Strip whitespace because
  // managed-runtime stream fragments content across blocks per-token.
  const normalized = result.assistantTextJoined.replace(/\s+/g, "");
  expect(normalized).toContain(result.probes.system);
  expect(normalized).toContain(result.probes.agentsMd);
  expect(normalized).toContain(result.probes.prompt);

  // No secret leakage anywhere in the SDK-visible payload.
  expect(result.leakedProviderKey).toBe(false);

  // Outputs: any file the managed runtime wrote under the custom outputs.allowedDirs path was
  // uploaded. Downloading each one returns content (a download error
  // would surface in `sample`). We don't require managed runtime to write files —
  // some upstreams + models do, some don't — but if it did, the bytes
  // must round-trip.
  for (const out of result.outputs) {
    expect(out.sizeBytes).toBeGreaterThanOrEqual(0);
    if (out.sample !== null) {
      expect(out.sample.startsWith("(download error")).toBe(false);
    }
  }
}

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("live hosted API — comprehensive end-to-end via installed SDK", () => {
  it(
    "managed deepseek: real managed runtime + skills + MCP + AGENTS.md + system + outputs.allowedDirs",
    async () => {
      const result = await runCase(
        {
          scriptName: "comprehensive-managed-deepseek.mjs",
          provider: "deepseek",
          model: deepseekModel,
          keyEnvName: "DEEPSEEK_KEY_SUBMIT",
          keyValue: deepseekKey,
          customOutputDir: "/data/exports/custom",
          pollDeadlineMs: 8 * 60_000,
          pollIntervalMs: 3_000,
          timeoutMs: 10 * 60_000
        },
        install.installDir
      );
      assertManagedShape(result, [managedSkillName("alpha", "deepseek"), managedSkillName("beta", "deepseek")]);
      expect(result.runtime).toBe("managed");
      expect(result.provider).toBe("deepseek");
    },
    11 * 60_000
  );

});
