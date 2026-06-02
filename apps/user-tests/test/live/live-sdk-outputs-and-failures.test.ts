/**
 * Live scenario: live-sdk-outputs-and-failures.test.ts
 *
 * Block A — outputs round-trip (matrix over cells)
 *   Prompts the agent to write a specific marker into a known filename
 *   inside its output directory, then asserts the bytes round-trip via
 *   listOutputs + downloadOutput. Catches:
 *     - upload silently truncates
 *     - opaque-output-id ↔ filename collisions
 *     - native-runtime output-capture not wired (today's gap)
 *
 * Block B — failure surfacing (single cell)
 *   Three sub-cases exercise the SDK's error contract:
 *     b1: corrupted skill zip → submit 4xx OR run "failed" with structured
 *         AntpathError/errorMessage
 *     b2: invalid model     → same accept-both shape
 *     b3: stdio MCP         → 4xx with REMOTE_MCP_STDIO_REJECTED_MESSAGE
 *
 *   Per AGENTS.md "No feature gates between runtimes" there's no
 *   capability-gate test — the no-gates rule means a native cell that
 *   silently fails MCP is already caught by live-sdk-mcp-invocation.
 *
 * Required env: same as other live-sdk-* files.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { expectStructuredError } from "@antpath/conformance";
import { installAntpath, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (outputs-and-failures): required env ${name} is missing.`);
  }
  return value;
}

const liveApiBase = requireEnv("ANTPATH_LIVE_API_BASE");
const apiToken = requireEnv("ANTPATH_LIVE_API_TOKEN");
const anthropicKey = requireEnv("ANTPATH_USER_TEST_ANTHROPIC_KEY");
const deepseekKey = requireEnv("ANTPATH_USER_TEST_DEEPSEEK_KEY");
const anthropicModel = process.env["ANTPATH_USER_TEST_ANTHROPIC_MODEL"] ?? "claude-haiku-4-5";
const deepseekModel = process.env["ANTPATH_USER_TEST_DEEPSEEK_MODEL"] ?? "deepseek-chat";

interface Cell {
  readonly id: string;
  readonly provider: "anthropic" | "deepseek";
  readonly runtime: "native" | "managed";
  readonly model: string;
  readonly keyEnvName: string;
  readonly keyValue: string;
}

const CELLS: readonly Cell[] = [
  { id: "anthropic-native",  provider: "anthropic", runtime: "native",  model: anthropicModel, keyEnvName: "ANTHROPIC_KEY_SUBMIT", keyValue: anthropicKey },
  { id: "anthropic-managed", provider: "anthropic", runtime: "managed", model: anthropicModel, keyEnvName: "ANTHROPIC_KEY_SUBMIT", keyValue: anthropicKey },
  { id: "deepseek-managed",  provider: "deepseek",  runtime: "managed", model: deepseekModel,  keyEnvName: "DEEPSEEK_KEY_SUBMIT",  keyValue: deepseekKey }
];

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

/* -------------------- Block A: outputs round-trip -------------------- */

interface OutputCaseResult {
  readonly runId: string;
  readonly runStatus: string;
  readonly runtime: string;
  readonly provider: string;
  readonly marker: string;
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly outputs: ReadonlyArray<{ filename: string | null; sizeBytes: number; downloadedLen: number; sample: string }>;
  readonly assistantTextJoined: string;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
}

function buildOutputScript(cell: Cell, marker: string): string {
  // Each runtime captures from a different path: Goose walks the
  // customer's outputDirs verbatim (after the workspaceRoot rerooting),
  // while Anthropic native only auto-registers writes under
  // /mnt/session/outputs/. Naming the path explicitly in the prompt is
  // the most reliable way to reach both — Goose handles
  // /workspace/outputs/<rel> directly, native's synthetic-first-message
  // remaps it to /mnt/session/outputs/<rel> via the materializer.
  const prompt =
    `Use your filesystem tools to create a file called \`report.txt\` ` +
    `inside the output directory at \`/workspace/outputs/report-folder/\`. ` +
    `The file's only contents must be the literal text: ${marker} ` +
    `(no newline, no extra characters). Then reply briefly that you wrote it.`;
  return `
    import { AntpathClient } from "antpath";

    const client = new AntpathClient({
      baseUrl: process.env.ANTPATH_API_BASE,
      apiToken: process.env.ANTPATH_API_TOKEN
    });

    const runId = await client.submitRun({
      provider: ${JSON.stringify(cell.provider)},
      runtime: ${JSON.stringify(cell.runtime)},
      model: ${JSON.stringify(cell.model)},
      prompt: ${JSON.stringify(prompt)},
      builtins: ["developer"],
      outputDirs: ["/workspace/outputs/report-folder"],
      secrets: { ${cell.provider}: { apiKey: process.env.${cell.keyEnvName} } },
      idempotencyKey: "outputs-${cell.id}-" + Date.now()
    });

    const deadline = Date.now() + 6 * 60_000;
    let run = null;
    while (Date.now() < deadline) {
      run = await client.getRun(runId);
      if (run.status === "succeeded" || run.status === "failed" || run.status === "cancelled") break;
      await new Promise((r) => setTimeout(r, 2_500));
    }
    if (!run || (run.status !== "succeeded" && run.status !== "failed" && run.status !== "cancelled")) {
      process.stderr.write(JSON.stringify({ kind: "timeout", run }, null, 2));
      process.exit(2);
    }

    const events = await client.listEvents(runId);
    const outputs = await client.listOutputs(runId);

    const downloaded = [];
    for (const out of outputs) {
      let downloadedLen = 0;
      let sample = "";
      try {
        const bytes = await client.downloadOutput(runId, out);
        const text = new TextDecoder().decode(bytes);
        downloadedLen = text.length;
        sample = text.slice(0, 512);
      } catch (err) {
        sample = "(download error: " + (err && err.message ? err.message : String(err)) + ")";
      }
      downloaded.push({
        filename: out.filename ?? null,
        sizeBytes: out.sizeBytes ?? 0,
        downloadedLen,
        sample
      });
    }

    const assistantTextJoined = events
      .filter((e) => e.type === "TEXT_MESSAGE_CONTENT")
      .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
      .join(" ");
    const terminal = events.find((e) => (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR"));
    const streamErrors = events
      .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "antpath.stream_error")
      .map((e) => (e.data && typeof e.data === "object" ? e.data : { unknown: true }));

    const result = {
      runId: runId,
      runStatus: run.status,
      runtime: run.runtime ?? "(missing)",
      provider: run.provider ?? "(missing)",
      marker: ${JSON.stringify(marker)},
      eventCount: events.length,
      eventKinds: events.map((e) => e.type),
      outputs: downloaded,
      assistantTextJoined,
      terminalKind: terminal ? terminal.type : null,
      terminalData: terminal ? terminal.data : null,
      streamErrors
    };
    process.stdout.write(JSON.stringify(result));
  `;
}

function dumpOutputResult(cell: Cell, result: OutputCaseResult): string {
  const lines: string[] = [];
  lines.push(`cell=${cell.id} runId=${result.runId} marker=${result.marker}`);
  lines.push(`runStatus=${result.runStatus} runtime=${result.runtime} provider=${result.provider}`);
  lines.push(`terminalKind=${result.terminalKind} terminalData=${JSON.stringify(result.terminalData)}`);
  lines.push(`eventKinds=[${result.eventKinds.join(", ")}]`);
  if (result.streamErrors.length > 0) {
    lines.push(`streamErrors:`);
    for (const se of result.streamErrors) {
      lines.push(`  - ${JSON.stringify(se).slice(0, 600)}`);
    }
  }
  lines.push(`outputs:`);
  for (const o of result.outputs) {
    lines.push(
      `  - filename=${o.filename} sizeBytes=${o.sizeBytes} downloadedLen=${o.downloadedLen} sample=${o.sample.slice(0, 200)}`
    );
  }
  lines.push(`assistantText=${result.assistantTextJoined.slice(0, 400)}`);
  return lines.join("\n");
}

async function runOutputCell(cell: Cell, installDir: string): Promise<OutputCaseResult> {
  const marker = `REPORT-${Math.random().toString(36).slice(2, 10).toUpperCase()}-EOF`;
  const script = buildOutputScript(cell, marker);
  const scriptPath = join(installDir, `outputs-${cell.id}.mjs`);
  writeFileSync(scriptPath, script);
  const passEnv = buildPassEnv({
    ANTPATH_API_BASE: liveApiBase,
    ANTPATH_API_TOKEN: apiToken,
    [cell.keyEnvName]: cell.keyValue
  });
  const child = await runCommand(process.execPath, [scriptPath], {
    cwd: installDir,
    timeoutMs: 8 * 60_000,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `outputs runner (${cell.id}) exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as OutputCaseResult;
}

/* -------------------- Block B: failure surfacing -------------------- */

interface FailureCaseResult {
  readonly kind: string;
  readonly submitOk: boolean;
  readonly submitStatus: number | null;
  readonly submitBody: string | null;
  readonly errorClass: string | null;
  readonly errorCode: string | null;
  readonly errorMessage: string | null;
  readonly runId: string | null;
  readonly runStatus: string | null;
  readonly runErrorMessage: string | null;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly eventKinds: readonly string[];
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
}

function buildCorruptedSkillScript(): string {
  // PKZIP end-of-central-directory record with empty payload — passes
  // magic-byte sniffing but is unparseable.
  return `
    import { AntpathClient } from "antpath";

    const client = new AntpathClient({
      baseUrl: process.env.ANTPATH_API_BASE,
      apiToken: process.env.ANTPATH_API_TOKEN
    });

    const corruptedZip = new Uint8Array([0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    let submitOk = false;
    let submitStatus = null;
    let submitBody = null;
    let errorClass = null;
    let errorCode = null;
    let errorMessage = null;
    let runId = null;
    let runStatus = null;
    let runErrorMessage = null;
    let terminalKind = null;
    let terminalData = null;
    let eventKinds = [];
    let streamErrors = [];

    // Stage the corrupted bytes directly via the raw /assets/upload
    // endpoint so we can submit a run that references a malformed
    // bundle — Skill.fromFiles() would build a VALID zip we can't
    // corrupt at the SDK layer.
    let skillRef = null;
    try {
      const hashBuf = await crypto.subtle.digest("SHA-256", corruptedZip);
      const hashHex = Array.from(new Uint8Array(hashBuf)).map((b) => b.toString(16).padStart(2, "0")).join("");
      const res = await fetch(process.env.ANTPATH_API_BASE + "/assets/upload", {
        method: "POST",
        headers: {
          "content-type": "application/octet-stream",
          "content-length": String(corruptedZip.byteLength),
          "x-asset-hash": "sha256:" + hashHex,
          authorization: "Bearer " + process.env.ANTPATH_API_TOKEN
        },
        body: corruptedZip
      });
      if (res.status === 200 || res.status === 201) {
        const body = await res.json();
        skillRef = { kind: "r2", path: body.path, hash: body.hash, sizeBytes: body.sizeBytes, name: "corrupt-skill" };
      } else {
        // Upload rejected — that's also a structured failure path. Record it.
        submitStatus = res.status;
        submitBody = (await res.text()).slice(0, 400);
        errorClass = "asset-upload-rejected";
        errorMessage = "upload rejected at status " + res.status;
      }
    } catch (e) {
      errorClass = e && e.constructor ? e.constructor.name : "Error";
      errorCode = e && typeof e.code === "string" ? e.code : null;
      errorMessage = e && e.message ? e.message : String(e);
    }

    if (skillRef) {
      try {
        runId = await client.submitRun({
          provider: "anthropic",
          runtime: "managed",
          model: ${JSON.stringify(anthropicModel)},
          prompt: "Hello.",
          skills: [skillRef],
          secrets: { anthropic: { apiKey: process.env.ANTHROPIC_KEY_SUBMIT } },
          idempotencyKey: "fail-corrupt-skill-" + Date.now()
        });
        submitOk = true;
        const deadline = Date.now() + 3 * 60_000;
        let run = null;
        while (Date.now() < deadline) {
          run = await client.getRun(runId);
          if (run.status === "succeeded" || run.status === "failed" || run.status === "cancelled") break;
          await new Promise((r) => setTimeout(r, 2_500));
        }
        runStatus = run ? run.status : null;
        runErrorMessage = run && run.errorMessage ? run.errorMessage : null;

        const events = await client.listEvents(runId);
        eventKinds = events.map((e) => e.type);
        const terminal = events.find((e) => (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR"));
        terminalKind = terminal ? terminal.type : null;
        terminalData = terminal ? terminal.data : null;
        streamErrors = events
          .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "antpath.stream_error")
          .map((e) => (e.data && typeof e.data === "object" ? e.data : { unknown: true }));
      } catch (e) {
        errorClass = e && e.constructor ? e.constructor.name : "Error";
        errorCode = e && typeof e.code === "string" ? e.code : null;
        errorMessage = e && e.message ? e.message : String(e);
      }
    }

    const result = {
      kind: "corrupted-skill",
      submitOk,
      submitStatus,
      submitBody,
      errorClass,
      errorCode,
      errorMessage,
      runId,
      runStatus,
      runErrorMessage,
      terminalKind,
      terminalData,
      eventKinds,
      streamErrors
    };
    process.stdout.write(JSON.stringify(result));
  `;
}

function buildIncompatibleRuntimeScript(): string {
  // Probe: provider="deepseek" + runtime="native". The hosted API MUST reject
  // at submit because no native runtime exists for deepseek (the parser
  // enforces this — selectRuntime + runtime_native_unsupported in
  // packages/contracts/src/submission.ts). This is a deterministic 4xx that
  // exercises the SDK's error-shape contract without depending on any
  // provider's tolerance for garbage model strings (the original b2 used
  // an invalid model string, which Anthropic silently accepted — a different
  // class of bug worth its own ticket, but unsuitable as an SDK
  // error-contract probe).
  return `
    import { AntpathClient } from "antpath";

    const client = new AntpathClient({
      baseUrl: process.env.ANTPATH_API_BASE,
      apiToken: process.env.ANTPATH_API_TOKEN
    });

    let submitOk = false;
    let submitStatus = null;
    let submitBody = null;
    let errorClass = null;
    let errorCode = null;
    let errorMessage = null;

    try {
      const runId = await client.submitRun({
        provider: "deepseek",
        runtime: "native",
        model: "deepseek-chat",
        prompt: "Hello.",
        secrets: { deepseek: { apiKey: process.env.DEEPSEEK_KEY_SUBMIT ?? "sk-test" } },
        idempotencyKey: "fail-incompat-runtime-" + Date.now()
      });
      submitOk = true;
    } catch (e) {
      errorClass = e && e.constructor ? e.constructor.name : "Error";
      errorCode = e && typeof e.code === "string" ? e.code : null;
      errorMessage = e && e.message ? e.message : String(e);
      // The SDK throws AntpathError; also expose the HTTP status if it
      // round-tripped through the structured error.
      if (e && typeof e.status === "number") submitStatus = e.status;
      if (e && typeof e.body === "string") submitBody = e.body.slice(0, 800);
    }

    const result = {
      kind: "incompatible-runtime",
      submitOk,
      submitStatus,
      submitBody,
      errorClass,
      errorCode,
      errorMessage,
      runId: null,
      runStatus: null,
      runErrorMessage: null,
      terminalKind: null,
      terminalData: null,
      eventKinds: [],
      streamErrors: []
    };
    process.stdout.write(JSON.stringify(result));
  `;
}

function buildStdioMcpScript(): string {
  // Sent via raw fetch — McpServer.remote() refuses transport:"stdio" at
  // the SDK builder layer (the surface is named ".remote"), so we send a
  // hand-crafted body to assert the hosted API 4xx with the canonical
  // REMOTE_MCP_STDIO_REJECTED_MESSAGE substring.
  return `
    let submitOk = false;
    let submitStatus = null;
    let submitBody = null;
    let errorClass = null;
    let errorCode = null;
    let errorMessage = null;

    try {
      const res = await fetch(process.env.ANTPATH_API_BASE + "/runs", {
        method: "POST",
        headers: {
          "content-type": "application/json",
          authorization: "Bearer " + process.env.ANTPATH_API_TOKEN
        },
        body: JSON.stringify({
          workspaceId: "ws-test",
          idempotencyKey: "fail-stdio-mcp-" + Date.now(),
          provider: "anthropic",
          runtime: "managed",
          submission: {
            model: ${JSON.stringify(anthropicModel)},
            prompt: ["Hello."],
            skills: [],
            agentsMd: [],
            files: [],
            mcpServers: [{ name: "bad-stdio", url: "stdio:///dev/null", transport: "stdio" }]
          },
          secrets: { anthropic: { apiKey: process.env.ANTHROPIC_KEY_SUBMIT } }
        })
      });
      submitStatus = res.status;
      submitBody = (await res.text()).slice(0, 800);
      submitOk = res.status >= 200 && res.status < 300;
    } catch (e) {
      errorClass = e && e.constructor ? e.constructor.name : "Error";
      errorCode = e && typeof e.code === "string" ? e.code : null;
      errorMessage = e && e.message ? e.message : String(e);
    }

    const result = {
      kind: "stdio-mcp",
      submitOk,
      submitStatus,
      submitBody,
      errorClass,
      errorCode,
      errorMessage,
      runId: null,
      runStatus: null,
      runErrorMessage: null,
      terminalKind: null,
      terminalData: null,
      eventKinds: [],
      streamErrors: []
    };
    process.stdout.write(JSON.stringify(result));
  `;
}

async function runFailureCase(
  scriptBuilder: () => string,
  scriptName: string,
  installDir: string
): Promise<FailureCaseResult> {
  const script = scriptBuilder();
  const scriptPath = join(installDir, scriptName);
  writeFileSync(scriptPath, script);
  const passEnv = buildPassEnv({
    ANTPATH_API_BASE: liveApiBase,
    ANTPATH_API_TOKEN: apiToken,
    ANTHROPIC_KEY_SUBMIT: anthropicKey
  });
  const child = await runCommand(process.execPath, [scriptPath], {
    cwd: installDir,
    timeoutMs: 5 * 60_000,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `failure runner (${scriptName}) exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as FailureCaseResult;
}

function dumpFailureResult(result: FailureCaseResult): string {
  return [
    `kind=${result.kind}`,
    `submitOk=${result.submitOk} submitStatus=${result.submitStatus}`,
    `submitBody=${(result.submitBody ?? "").slice(0, 400)}`,
    `errorClass=${result.errorClass} errorCode=${result.errorCode}`,
    `errorMessage=${(result.errorMessage ?? "").slice(0, 400)}`,
    `runId=${result.runId} runStatus=${result.runStatus}`,
    `runErrorMessage=${(result.runErrorMessage ?? "").slice(0, 400)}`,
    `terminalKind=${result.terminalKind} terminalData=${JSON.stringify(result.terminalData)}`,
    `eventKinds=[${result.eventKinds.join(", ")}]`,
    `streamErrors=${JSON.stringify(result.streamErrors).slice(0, 600)}`
  ].join("\n");
}

/* ------------------------------- Tests ------------------------------- */

let install: InstallResult;

beforeAll(async () => {
  install = await installAntpath();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("live outputs — agent writes a known file, bytes round-trip", () => {
  it.each(CELLS)(
    "$id: agent writes report.txt with marker, listOutputs + download recovers it",
    async (cell) => {
      const result = await runOutputCell(cell, install.installDir);
      const dump = (): string => dumpOutputResult(cell, result);

      expect(result.runStatus, dump()).toBe("succeeded");
      expect(result.runtime).toBe(cell.runtime);
      expect(result.provider).toBe(cell.provider);
      expect(result.eventKinds).toContain("RUN_STARTED");
      expect(result.terminalKind).toBe("RUN_FINISHED");
      // Every clean terminal MUST carry reason="complete" — both adapters
      // always populate reason on the success path. Tolerating `undefined`
      // (pre-Phase-1) was masking field-loss regressions.
      const terminalReason = result.terminalData ? result.terminalData["reason"] : undefined;
      if (terminalReason !== "complete") {
        throw new Error(`terminal reason=${terminalReason} (expected "complete")\n\n${dump()}`);
      }

      // Find the report.txt the agent wrote. Filename is relative to
      // workspaceRoot (/workspace) so an agent writing to
      // /workspace/outputs/report-folder/report.txt yields filename
      // "outputs/report-folder/report.txt". Asserting on endsWith keeps
      // the test robust to the model drifting one directory level.
      const reportFile = result.outputs.find(
        (o) => o.filename && o.filename.endsWith("report.txt")
      );
      if (!reportFile) {
        throw new Error(`no output filename endsWith "report.txt"\n\n${dump()}`);
      }

      // Bytes round-trip — content matches marker, sizeBytes matches
      // downloaded length.
      if (!reportFile.sample.includes(result.marker)) {
        throw new Error(
          `downloaded report.txt missing marker "${result.marker}"\n\n${dump()}`
        );
      }
      if (reportFile.sizeBytes !== reportFile.downloadedLen) {
        throw new Error(
          `sizeBytes (${reportFile.sizeBytes}) !== downloadedLen (${reportFile.downloadedLen}) — upload truncation suspected\n\n${dump()}`
        );
      }
    },
    10 * 60_000
  );
});

describe("live failure surfacing — SDK error contract", () => {
  it(
    "b1 corrupted skill zip: 4xx at submit OR run status:failed with structured error",
    async () => {
      const result = await runFailureCase(
        buildCorruptedSkillScript,
        "fail-corrupted-skill.mjs",
        install.installDir
      );
      const dump = (): string => dumpFailureResult(result);

      // Accept either branch:
      //  - submit threw with a structured AntpathError (errorClass non-null)
      //  - submit succeeded but the run reached terminal "failed" with
      //    a populated errorMessage (or runtime_terminal carrying reason!=
      //    "complete" + a stream_error event)
      if (!result.submitOk) {
        // upload or submit rejected. Either way, the error must be
        // structured — non-empty errorMessage, structured errorClass.
        if (!result.errorMessage || result.errorMessage.length === 0) {
          throw new Error(`submit rejected but no errorMessage\n\n${dump()}`);
        }
        if (!result.errorClass || result.errorClass === "Object") {
          throw new Error(`submit rejected but errorClass is not structured\n\n${dump()}`);
        }
      } else {
        // submit accepted; the run must have failed terminally with a
        // reported error. The structured failure can surface as
        // Run.errorMessage OR terminalData.failureMessage — both are
        // legitimate places the API plumbs the cause; require at least
        // one to be populated so a silent failure is caught.
        if (result.runStatus !== "failed") {
          throw new Error(
            `submit accepted but run did not fail (status=${result.runStatus})\n\n${dump()}`
          );
        }
        const terminalFailureMsg =
          result.terminalData && typeof result.terminalData["failureMessage"] === "string"
            ? (result.terminalData["failureMessage"] as string)
            : "";
        const haveStructuredCause =
          (result.runErrorMessage && result.runErrorMessage.length > 0) ||
          terminalFailureMsg.length > 0;
        if (!haveStructuredCause) {
          throw new Error(
            `run failed but no errorMessage on Run AND no terminalData.failureMessage\n\n${dump()}`
          );
        }
        // A runtime_terminal with reason!="complete" is the on-stream
        // signal that the run did not finish cleanly. Either reason ===
        // "error" OR a stream_error event must be present.
        const terminalReason = result.terminalData ? result.terminalData["reason"] : undefined;
        const sawSignal =
          (typeof terminalReason === "string" && terminalReason !== "complete") ||
          result.streamErrors.length > 0;
        if (!sawSignal) {
          throw new Error(
            `run failed but no terminal reason!=complete and no stream_error events\n\n${dump()}`
          );
        }
      }
    },
    7 * 60_000
  );

  it(
    "b2 incompatible provider/runtime: rejected at submit with structured error class",
    async () => {
      // The ORIGINAL b2 used an invalid model string and was removed
      // because Anthropic silently accepted placeholder models — a
      // separate product gap, not an SDK error-contract bug. The
      // reinstated b2 probes a deterministic 4xx that NO provider can
      // mask: provider="deepseek" + runtime="native" is rejected by the
      // submission parser itself (runtime_native_unsupported).
      const result = await runFailureCase(
        buildIncompatibleRuntimeScript,
        "fail-incompat-runtime.mjs",
        install.installDir
      );
      const dump = (): string => dumpFailureResult(result);

      // Hosted API MUST reject at submit.
      expect(result.submitOk, dump()).toBe(false);
      // SDK MUST surface a structured error class containing a
      // "runtime"/"native" hint so the customer can self-diagnose. The
      // canonical class is AntpathError; the canonical code is
      // runtime_native_unsupported (packages/contracts/src/submission.ts).
      // The shared matcher pins both — a regression that drops the class
      // (errorClass=null) or the substring would have to weaken the
      // helper, which is visible across every consumer in code review.
      try {
        expectStructuredError(
          {
            errorClass: result.errorClass,
            errorCode: result.errorCode,
            errorMessage: result.errorMessage
          },
          { messageIncludes: "runtime", context: "b2 incompatible-runtime" }
        );
      } catch (e) {
        // Try the alternate hint word ("native") before giving up — the
        // SDK message is one of two canonical phrasings.
        expectStructuredError(
          {
            errorClass: result.errorClass,
            errorCode: result.errorCode,
            errorMessage: result.errorMessage
          },
          { messageIncludes: "native", context: "b2 incompatible-runtime (alt hint)" }
        );
      }
    },
    3 * 60_000
  );

  it(
    "b3 stdio MCP: rejected at submit with REMOTE_MCP_STDIO_REJECTED_MESSAGE",
    async () => {
      const result = await runFailureCase(
        buildStdioMcpScript,
        "fail-stdio-mcp.mjs",
        install.installDir
      );
      const dump = (): string => dumpFailureResult(result);

      // Hosted API MUST reject at submit. No accept-both branch here.
      expect(result.submitOk, dump()).toBe(false);
      expect(result.submitStatus, dump()).toBeGreaterThanOrEqual(400);
      expect(result.submitStatus, dump()).toBeLessThan(500);

      // The error body MUST identify this as stdio-rejection so a user
      // who hits this gets an actionable message. The canonical
      // message lives in shared run-submission validation — full
      // text or unique fragment ("stdio") must appear.
      const body = result.submitBody ?? "";
      if (!body.toLowerCase().includes("stdio")) {
        throw new Error(
          `expected 4xx body to mention "stdio"; got: ${body.slice(0, 400)}\n\n${dump()}`
        );
      }
    },
    3 * 60_000
  );
});
