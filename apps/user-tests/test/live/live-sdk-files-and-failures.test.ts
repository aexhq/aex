/**
 * Live scenario: live-sdk-files-and-failures.test.ts
 *
 * Block A — files round-trip (matrix over cells)
 *   Prompts the agent to write a specific marker into a known filename
 *   inside its files directory, then asserts the bytes round-trip via
 *   listFiles + downloadSessionFile. Catches:
 *     - upload silently truncates
 *     - opaque-file-id ↔ filename collisions
 *     - managed runtime file-capture not wired
 *
 * Block B — failure surfacing (single cell)
 *   Three sub-cases exercise the SDK's error contract:
 *     b1: corrupted skill zip -> submit 4xx OR session lifecycle `error` with structured
 *         AexError/errorMessage
 *     b2: invalid model     → same accept-both shape
 *     b3: stdio MCP         → 4xx with REMOTE_MCP_STDIO_REJECTED_MESSAGE
 *
 *   The MCP invocation scenario covers tool-routing regressions.
 *
 * Required env: same as other live-sdk-* files.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { expectStructuredError } from "../_fixtures/structured-error.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (files-and-failures): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek/deepseek-v4-flash";

interface Cell {
  readonly id: string;
  readonly provider: "deepseek";
  readonly model: string;
}

const CELLS: readonly Cell[] = [
  { id: "deepseek-managed", provider: "deepseek", model: deepseekModel }
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

/* -------------------- Block A: files round-trip -------------------- */

interface FileCaseResult {
  readonly sessionId: string;
  readonly runStatus: string;
  readonly runtime: string;
  readonly provider: string;
  readonly marker: string;
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly files: ReadonlyArray<{ filename: string | null; sizeBytes: number; downloadedLen: number; sample: string }>;
  readonly assistantTextJoined: string;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
}

function buildFileScript(cell: Cell, marker: string): string {
  // This case narrows capture to an explicit fileCapture.allowedDirs root. Naming the
  // path explicitly in the prompt avoids model variance around path choice.
  const prompt =
    `Use your filesystem tools to create a file called \`report.txt\` ` +
    `inside \`/workspace/files/report-folder/\`. ` +
    `The file's only contents must be the literal text: ${marker} ` +
    `(no newline, no extra characters). Then reply briefly that you wrote it.`;
  return `
    import { Aex } from "@aexhq/sdk";

    const client = new Aex({
      baseUrl: process.env.AEX_API_URL,
      apiKey: process.env.AEX_API_KEY
    });

    const sessionResult = await client.start({
      model: ${JSON.stringify(cell.model)},
      message: ${JSON.stringify(prompt)},
      builtinTools: "default",
      fileCapture: { allowedDirs: ["/workspace/files/report-folder"] },
      idempotencyKey: "files-${cell.id}-" + Date.now()
    }, { timeoutMs: 6 * 60_000 });
    const sessionId = sessionResult.sessionId;
    const session = await client.sessions.open(sessionId);
    const sessionInfo = {
      status: sessionResult.status,
      runtime: "managed",
    };
    const events = (await session.events.list()).filter((event) => event.runId === sessionResult.run.runId);
    const files = (await session.files.list()).files;

    const downloaded = [];
    for (const out of files) {
      let downloadedLen = 0;
      let sample = "";
      try {
        const bytes = await session.files.download(out);
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
    const terminal = events.find((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR");
    const eventKinds = events.map((e) => e.type);
    const terminalData = terminal ? terminal.data : null;
    const streamErrors = events
      .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error")
      .map((e) => (e.data && typeof e.data === "object" ? e.data : { unknown: true }));

    const result = {
      sessionId: sessionId,
      runStatus: sessionInfo.status,
      runtime: sessionInfo.runtime ?? "(missing)",
      provider: sessionInfo.provider ?? "(missing)",
      marker: ${JSON.stringify(marker)},
      eventCount: events.length,
      eventKinds,
      files: downloaded,
      assistantTextJoined,
      terminalKind: terminal ? terminal.type : null,
      terminalData,
      streamErrors
    };
    process.stdout.write(JSON.stringify(result));
    process.exit(0);
  `;
}

function dumpFileResult(cell: Cell, result: FileCaseResult): string {
  const lines: string[] = [];
  lines.push(`cell=${cell.id} sessionId=${result.sessionId} marker=${result.marker}`);
  lines.push(`runStatus=${result.runStatus} runtime=${result.runtime} provider=${result.provider}`);
  lines.push(`terminalKind=${result.terminalKind} terminalData=${JSON.stringify(result.terminalData)}`);
  lines.push(`eventKinds=[${result.eventKinds.join(", ")}]`);
  if (result.streamErrors.length > 0) {
    lines.push(`streamErrors:`);
    for (const se of result.streamErrors) {
      lines.push(`  - ${JSON.stringify(se).slice(0, 600)}`);
    }
  }
  lines.push(`files:`);
  for (const o of result.files) {
    lines.push(
      `  - filename=${o.filename} sizeBytes=${o.sizeBytes} downloadedLen=${o.downloadedLen} sample=${o.sample.slice(0, 200)}`
    );
  }
  lines.push(`assistantText=${result.assistantTextJoined.slice(0, 400)}`);
  return lines.join("\n");
}

function assertCleanFileLifecycle(cell: Cell, result: FileCaseResult): void {
  const dump = (): string => dumpFileResult(cell, result);
  if (result.streamErrors.length > 0) {
    throw new Error(`clean files session emitted stream errors\n\n${dump()}`);
  }
  const files = result.terminalData ? result.terminalData["files"] : undefined;
  if (files !== undefined) {
    if (!files || typeof files !== "object" || Array.isArray(files)) {
      throw new Error(`terminal files summary is malformed\n\n${dump()}`);
    }
    const summary = files as Record<string, unknown>;
    for (const field of ["uploaded", "skipped", "failures", "droppedByCap", "totalBytes"] as const) {
      const value = summary[field];
      if (typeof value !== "number" || !Number.isFinite(value)) {
        throw new Error(`terminal files.${field} must be a finite number\n\n${dump()}`);
      }
    }
    if ((summary["uploaded"] as number) < 1 || summary["failures"] !== 0 || summary["droppedByCap"] !== 0) {
      throw new Error(`unexpected terminal files summary on clean session: ${JSON.stringify(summary)}\n\n${dump()}`);
    }
  }
}

async function startFileCell(cell: Cell, installDir: string): Promise<FileCaseResult> {
  const marker = `REPORT-${Math.random().toString(36).slice(2, 10).toUpperCase()}-EOF`;
  const script = buildFileScript(cell, marker);
  const scriptPath = join(installDir, `files-${cell.id}.mjs`);
  writeFileSync(scriptPath, script);
  const passEnv = buildPassEnv({
    AEX_API_URL: apiUrl,
    AEX_API_KEY: apiKey
  });
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: installDir,
    timeoutMs: 8 * 60_000,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `files runner (${cell.id}) exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as FileCaseResult;
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
  readonly sessionId: string | null;
  readonly sessionStatus: string | null;
  readonly sessionPollStatus: number | null;
  readonly sessionPollAttempts: number;
  readonly sessionErrorMessage: string | null;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly eventKinds: readonly string[];
  readonly streamErrors: ReadonlyArray<Record<string, unknown>>;
}

function buildCorruptedSkillScript(): string {
  // PKZIP end-of-central-directory record with no entries — valid enough to
  // upload, but not a skill because it has no root SKILL.md. Submitted via raw
  // fetch (no SDK Skill factory), since Skill.fromBytes() would reject before the
  // malformed bundle can exercise the hosted materialization path.
  return `
    const corruptedZip = new Uint8Array([0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

    let submitOk = false;
    let submitStatus = null;
    let submitBody = null;
    let errorClass = null;
    let errorCode = null;
    let errorMessage = null;
    let sessionId = null;
    let acceptedRunId = null;
    let sessionStatus = null;
    let sessionPollStatus = null;
    let sessionPollAttempts = 0;
    let sessionErrorMessage = null;
    let terminalKind = null;
    let terminalData = null;
    let eventKinds = [];
    let streamErrors = [];

    // Stage the corrupted bytes through the supported asset path, then bind that
    // content hash to a workspace skill name. Skill.fromBytes()/fromDir would
    // build or validate a good skill bundle, so this raw setup is the only way
    // to submit a first-class skill whose bytes are malformed.
    let corruptSkillRef = null;
    try {
      const hashBuf = await crypto.subtle.digest("SHA-256", corruptedZip);
      const hashHex = Array.from(new Uint8Array(hashBuf)).map((b) => b.toString(16).padStart(2, "0")).join("");
      const contentHash = "sha256:" + hashHex;

      const presignRes = await fetch(process.env.AEX_API_URL + "/api/assets/presign", {
        method: "POST",
        headers: {
          "content-type": "application/json",
          authorization: "Bearer " + process.env.AEX_API_KEY
        },
        body: JSON.stringify({ hash: contentHash, sizeBytes: corruptedZip.byteLength, contentType: "application/zip" })
      });
      const presignText = await presignRes.text();
      submitStatus = presignRes.status;
      submitBody = presignText.slice(0, 800);
      if (!presignRes.ok) {
        errorClass = "asset-presign-rejected";
        errorMessage = "asset presign rejected at status " + presignRes.status + ": " + submitBody.slice(0, 200);
      } else {
        const presign = JSON.parse(presignText);
        if (presign.uploadUrl) {
          const putRes = await fetch(presign.uploadUrl, {
            method: "PUT",
            headers: {
              "content-type": "application/zip",
              ...(presign.requiredHeaders && typeof presign.requiredHeaders === "object" ? presign.requiredHeaders : {})
            },
            body: corruptedZip
          });
          if (!putRes.ok) {
            const putText = await putRes.text().catch(() => "");
            errorClass = "asset-upload-rejected";
            errorMessage = "direct asset upload rejected at status " + putRes.status + ": " + putText.slice(0, 200);
          }
        }
        if (!errorClass) {
          const finalizeRes = await fetch(process.env.AEX_API_URL + "/api/assets/finalize", {
            method: "POST",
            headers: {
              "content-type": "application/json",
              authorization: "Bearer " + process.env.AEX_API_KEY
            },
            body: JSON.stringify({ hash: contentHash, sizeBytes: corruptedZip.byteLength })
          });
          const finalizeText = await finalizeRes.text();
          submitStatus = finalizeRes.status;
          submitBody = finalizeText.slice(0, 800);
          if (!finalizeRes.ok) {
            errorClass = "asset-finalize-rejected";
            errorMessage = "asset finalize rejected at status " + finalizeRes.status + ": " + submitBody.slice(0, 200);
          }
        }
        if (!errorClass) {
          const skillName = "corrupt-skill-" + Date.now();
          const assetId = "asset_" + hashHex;
          const publishRes = await fetch(process.env.AEX_API_URL + "/api/workspace/skills", {
            method: "POST",
            headers: {
              "content-type": "application/json",
              authorization: "Bearer " + process.env.AEX_API_KEY
            },
            body: JSON.stringify({
              assetId,
              contentHash,
              description: "Corrupted skill bundle probe.",
              sizeBytes: corruptedZip.byteLength,
              contentType: "application/zip",
              name: skillName
            })
          });
          const publishText = await publishRes.text();
          submitStatus = publishRes.status;
          submitBody = publishText.slice(0, 800);
          if (publishRes.ok) {
            const published = JSON.parse(publishText);
            corruptSkillRef = published.resource;
          } else {
            errorClass = "skill-publish-rejected";
            errorMessage = "skill publish rejected at status " + publishRes.status + ": " + submitBody.slice(0, 200);
          }
        }
      }
    } catch (e) {
      errorClass = e && e.constructor ? e.constructor.name : "Error";
      errorCode = e && typeof e.code === "string" ? e.code : null;
      errorMessage = e && e.message ? e.message : String(e);
    }

    if (corruptSkillRef) {
      try {
        // Submit the immutable workspace skill ref. Materialization must reject
        // the malformed archive at create or run time.
        const res = await fetch(process.env.AEX_API_URL + "/api/sessions", {
          method: "POST",
          headers: {
            "content-type": "application/json",
            authorization: "Bearer " + process.env.AEX_API_KEY,
            "Idempotency-Key": "fail-corrupt-skill-" + Date.now()
          },
          body: JSON.stringify({
            submission: {
              model: ${JSON.stringify(deepseekModel)},
              assets: { files: [], skills: [corruptSkillRef], tools: [], instructions: [] },
              builtinTools: "default",
              mcpServers: []
            },
            retention: { idleTtl: "3m" },
          })
        });
        submitStatus = res.status;
        const submitText = await res.text();
        submitBody = submitText.slice(0, 800);
        submitOk = res.status === 200 || res.status === 201;
        if (!submitOk) {
          errorClass = "session-submit-rejected";
          errorMessage = "corrupted skill bundle rejected at status " + res.status + ": " + submitBody.slice(0, 200);
        } else {
          try {
            const accepted = JSON.parse(submitText);
            const acceptedSession = accepted && accepted.session && typeof accepted.session === "object" ? accepted.session : null;
            sessionId = acceptedSession && typeof acceptedSession.id === "string" ? acceptedSession.id : null;
          } catch {
            sessionId = null;
          }
          if (!sessionId) {
            errorClass = "session-submit-missing-id";
            errorMessage = "corrupted skill bundle submit was accepted but returned no session id: " + submitBody.slice(0, 200);
          }
          if (sessionId) {
            const messageRes = await fetch(process.env.AEX_API_URL + "/api/sessions/" + encodeURIComponent(sessionId) + "/messages", {
              method: "POST",
              headers: {
                "content-type": "application/json",
                authorization: "Bearer " + process.env.AEX_API_KEY,
                "Idempotency-Key": "fail-corrupt-skill-message-" + Date.now()
              },
              body: JSON.stringify({ input: "Hello." })
            });
            const messageText = await messageRes.text();
            submitStatus = messageRes.status;
            submitBody = messageText.slice(0, 800);
            if (messageRes.status !== 202) {
              submitOk = false;
              errorClass = "session-message-rejected";
              errorMessage = "corrupted skill bundle message rejected at status " + messageRes.status + ": " + messageText.slice(0, 200);
            } else {
              const acceptedMessage = JSON.parse(messageText);
              acceptedRunId = acceptedMessage && acceptedMessage.run && typeof acceptedMessage.run.runId === "string"
                ? acceptedMessage.run.runId
                : null;
              if (!acceptedRunId) throw new Error("message acceptance omitted run.runId");
            }
          }
        }
      } catch (e) {
        errorClass = e && e.constructor ? e.constructor.name : "Error";
        errorCode = e && typeof e.code === "string" ? e.code : null;
        errorMessage = e && e.message ? e.message : String(e);
      }
    }

    if (submitOk && sessionId && acceptedRunId) {
      let terminal = null;
      let events = [];
      const authHeaders = { authorization: "Bearer " + process.env.AEX_API_KEY };
      const deadline = Date.now() + Number(process.env.FAILURE_WAIT_MS || "120000");
      while (!terminal && Date.now() < deadline) {
        const eventsRes = await fetch(process.env.AEX_API_URL + "/api/sessions/" + encodeURIComponent(sessionId) + "/events?limit=1000", {
          headers: authHeaders
        });
        if (!eventsRes.ok) throw new Error("event list failed with " + eventsRes.status);
        const eventsBody = await eventsRes.json();
        events = Array.isArray(eventsBody.events) ? eventsBody.events : [];
        terminal = events.find((event) => event && (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR")) ?? null;
        if (!terminal) await new Promise((resolve) => setTimeout(resolve, 1000));
      }
      if (!terminal) throw new Error("corrupted skill run ended without a terminal event");
      const sessionRes = await fetch(process.env.AEX_API_URL + "/api/sessions/" + encodeURIComponent(sessionId), {
        headers: authHeaders
      });
      sessionPollStatus = sessionRes.status;
      if (!sessionRes.ok) throw new Error("session refresh failed with " + sessionRes.status);
      const sessionBody = await sessionRes.json();
      if (!sessionBody || typeof sessionBody.session !== "object" || sessionBody.session === null) {
        throw new Error("session refresh response omitted session envelope");
      }
      const sessionRecord = sessionBody.session;
      sessionStatus = sessionRecord.status;
      sessionErrorMessage = typeof sessionRecord.errorMessage === "string" ? sessionRecord.errorMessage : null;
      sessionPollAttempts = 1;
      eventKinds = events.map((event) => event.type);
      terminalKind = terminal.type;
      terminalData = terminal.data;
      streamErrors = events
        .filter((event) => event.type === "CUSTOM" && event.data && event.data.name === "aex.stream_error")
        .map((event) => event.data && typeof event.data.value === "object" ? event.data.value : event.data);
    }

    const result = {
      kind: "corrupted-skill",
      submitOk,
      submitStatus,
      submitBody,
      errorClass,
      errorCode,
      errorMessage,
      sessionId,
      sessionStatus,
      sessionPollStatus,
      sessionPollAttempts,
      sessionErrorMessage,
      terminalKind,
      terminalData,
      eventKinds,
      streamErrors
    };
    process.stdout.write(JSON.stringify(result));
    process.exit(0);
  `;
}

function buildIncompatibleRuntimeScript(): string {
  // Probe: the legacy "runtimeSize" selector is gone (renamed to "runtime").
  // The installed SDK must reject the removed field before any HTTP call,
  // giving a deterministic error-shape check ("runtimeSize is not a supported
  // option") without depending on provider behavior.
  return `
    import { Aex } from "@aexhq/sdk";

    const client = new Aex({
      baseUrl: process.env.AEX_API_URL,
      apiKey: process.env.AEX_API_KEY
    });

    let submitOk = false;
    let submitStatus = null;
    let submitBody = null;
    let errorClass = null;
    let errorCode = null;
    let errorMessage = null;

    try {
      const result = await client.start({
        runtimeSize: "native",
        model: "deepseek/deepseek-v4-flash",
        message: "Hello.",
        idempotencyKey: "fail-incompat-runtime-" + Date.now()
      });
      void result;
      submitOk = true;
    } catch (e) {
      errorClass = e && e.constructor ? e.constructor.name : "Error";
      errorCode = e && typeof e.code === "string" ? e.code : null;
      errorMessage = e && e.message ? e.message : String(e);
      // The SDK throws AexError; also expose the HTTP status if it
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
      sessionId: null,
      sessionStatus: null,
      sessionPollStatus: null,
      sessionPollAttempts: 0,
      sessionErrorMessage: null,
      terminalKind: null,
      terminalData: null,
      eventKinds: [],
      streamErrors: []
    };
    process.stdout.write(JSON.stringify(result));
    process.exit(0);
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
      const res = await fetch(process.env.AEX_API_URL + "/api/sessions", {
        method: "POST",
        headers: {
          "content-type": "application/json",
          authorization: "Bearer " + process.env.AEX_API_KEY,
          "Idempotency-Key": "fail-stdio-mcp-" + Date.now()
        },
        body: JSON.stringify({
          submission: {
            model: ${JSON.stringify(deepseekModel)},
            assets: { files: [], skills: [], tools: [], instructions: [] },
            builtinTools: "default",
            mcpServers: [{ name: "bad-stdio", url: "stdio:///dev/null", transport: "stdio" }]
          },
          retention: { idleTtl: "3m" },
        })
      });
      submitStatus = res.status;
      submitBody = (await res.text()).slice(0, 800);
      submitOk = res.status === 200 || res.status === 201;
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
      sessionId: null,
      sessionStatus: null,
      sessionPollStatus: null,
      sessionPollAttempts: 0,
      sessionErrorMessage: null,
      terminalKind: null,
      terminalData: null,
      eventKinds: [],
      streamErrors: []
    };
    process.stdout.write(JSON.stringify(result));
    process.exit(0);
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
    AEX_API_URL: apiUrl,
    AEX_API_KEY: apiKey
  });
  const child = await runCommand(getBunCommand(), [scriptPath], {
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
    `sessionId=${result.sessionId} sessionStatus=${result.sessionStatus} sessionPollStatus=${result.sessionPollStatus} sessionPollAttempts=${result.sessionPollAttempts}`,
    `sessionErrorMessage=${(result.sessionErrorMessage ?? "").slice(0, 400)}`,
    `terminalKind=${result.terminalKind} terminalData=${JSON.stringify(result.terminalData)}`,
    `eventKinds=[${result.eventKinds.join(", ")}]`,
    `streamErrors=${JSON.stringify(result.streamErrors).slice(0, 600)}`
  ].join("\n");
}

/* ------------------------------- Tests ------------------------------- */

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("live files — agent writes a known file, bytes round-trip", () => {
  it.each([...CELLS])(
    "$id: agent writes report.txt with marker, listFiles + download recovers it",
    async (cell) => {
      const result = await startFileCell(cell, install.installDir);
      const dump = (): string => dumpFileResult(cell, result);

      expect(result.runStatus, dump()).toBe("succeeded");
      expect(result.runtime).toBe("managed");
      expect(result.provider).toBe(cell.provider);
      expect(result.terminalKind).toBe("RUN_FINISHED");
      expect(result.terminalData?.["outcome"], dump()).toBe("succeeded");
      assertCleanFileLifecycle(cell, result);

      // Find the report.txt the agent wrote. Filename is relative to
      // workspaceRoot (/workspace) so an agent writing to
      // /workspace/files/report-folder/report.txt yields filename
      // "files/report-folder/report.txt". Asserting on endsWith keeps
      // the test robust to the model drifting one directory level.
      const reportFile = result.files.find(
        (o) => o.filename && o.filename.endsWith("report.txt")
      );
      if (!reportFile) {
        throw new Error(`no session filename endsWith "report.txt"\n\n${dump()}`);
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
    "b1 corrupted skill zip: 4xx at submit OR RUN_ERROR with structured failure",
    async () => {
      const result = await runFailureCase(
        buildCorruptedSkillScript,
        "fail-corrupted-skill.mjs",
        install.installDir
      );
      const dump = (): string => dumpFailureResult(result);

      // Accept either branch:
      //  - submit threw with a structured AexError (errorClass non-null)
      //  - submit succeeded but the run emitted RUN_ERROR and the resumable
      //    session entered the `error` lifecycle state.
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
        // submit accepted; the session must have failed terminally with a
        // reported error. The structured failure can surface as
        // Session.errorMessage OR terminalData.failureMessage — both are
        // legitimate places the API plumbs the cause; require at least
        // one to be populated so a silent failure is caught.
        if (result.sessionStatus !== "error") {
          throw new Error(
            `submit accepted but session did not enter error (status=${result.sessionStatus})\n\n${dump()}`
          );
        }
        expect(result.terminalKind, dump()).toBe("RUN_ERROR");
        expect(result.terminalData?.["outcome"], dump()).toBe("failed");
        const terminalFailureMsg =
          result.terminalData && typeof result.terminalData["failureMessage"] === "string"
            ? (result.terminalData["failureMessage"] as string)
            : "";
        const haveStructuredCause =
          (result.sessionErrorMessage && result.sessionErrorMessage.length > 0) ||
          terminalFailureMsg.length > 0;
        if (!haveStructuredCause) {
          throw new Error(
            `session failed but no errorMessage on Session AND no terminalData.failureMessage\n\n${dump()}`
          );
        }
      }
    },
    7 * 60_000
  );

  it(
    "b2 invalid runtime selector: rejected at submit with structured error class",
    async () => {
      // The ORIGINAL b2 used an invalid model string and was removed because
      // Anthropic silently accepted placeholder models. This replacement probes
      // a deterministic SDK-side rejection: the legacy "runtimeSize" field is a
      // removed option and is rejected before any HTTP call.
      const result = await runFailureCase(
        buildIncompatibleRuntimeScript,
        "fail-incompat-runtime.mjs",
        install.installDir
      );
      const dump = (): string => dumpFailureResult(result);

      // The installed SDK MUST reject before submit.
      expect(result.submitOk, dump()).toBe(false);
      // SDK MUST surface a structured error class containing a
      // "runtime"/"native" hint so the customer can self-diagnose.
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
          {
            classes: ["AexError", "SessionConfigValidationError"],
            messageIncludes: "runtime",
            context: "b2 incompatible-runtime"
          }
        );
      } catch (e) {
        // Try the alternate hint word ("native") before giving up.
        expectStructuredError(
          {
            errorClass: result.errorClass,
            errorCode: result.errorCode,
            errorMessage: result.errorMessage
          },
          {
            classes: ["AexError", "SessionConfigValidationError"],
            messageIncludes: "native",
            context: "b2 incompatible-runtime (alt hint)"
          }
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
      expect(typeof result.submitStatus, dump()).toBe("number");
      const submitStatus = result.submitStatus as number;
      expect(submitStatus, dump()).toBeGreaterThanOrEqual(400);
      expect(submitStatus, dump()).toBeLessThan(500);

      // The error body MUST identify this as stdio-rejection so a user
      // who hits this gets an actionable message. The canonical
      // message lives in shared session-submission validation — full
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
