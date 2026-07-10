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
 *     b1: corrupted skill zip -> submit 4xx OR session "failed" with structured
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
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { expectStructuredError } from "@aexhq/conformance";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (files-and-failures): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

interface Cell {
  readonly id: string;
  readonly provider: "deepseek";
  readonly model: string;
  readonly keyEnvName: string;
  readonly keyValue: string;
}

const CELLS: readonly Cell[] = [
  { id: "deepseek-managed",  provider: "deepseek", model: deepseekModel,  keyEnvName: "DEEPSEEK_KEY_SUBMIT",  keyValue: deepseekKey }
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
  readonly sessionStatus: string;
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
      provider: ${JSON.stringify(cell.provider)},
      model: ${JSON.stringify(cell.model)},
      message: ${JSON.stringify(prompt)},
      includeBuiltinTools: true,
      fileCapture: { allowedDirs: ["/workspace/files/report-folder"] },
      apiKeys: { [${JSON.stringify(cell.provider)}]: process.env.${cell.keyEnvName} },
      idempotencyKey: "files-${cell.id}-" + Date.now()
    }, { timeoutMs: 6 * 60_000 });
    const sessionId = sessionResult.sessionId;
    const session = await client.sessions.open(sessionId);
    const sessionInfo = {
      status: sessionResult.ok ? "succeeded" : (typeof sessionResult.status === "string" && sessionResult.status ? sessionResult.status : "failed"),
      runtime: "managed",
      provider: ${JSON.stringify(cell.provider)}
    };
    const fallbackEvents = Array.isArray(sessionResult.events) ? sessionResult.events : [];
    const fallbackFiles = Array.isArray(sessionResult.files) ? sessionResult.files : [];
    let events = fallbackEvents;
    let files = fallbackFiles;
    try {
      const listedEvents = await session.events().list();
      if (Array.isArray(listedEvents) && listedEvents.length > 0) events = listedEvents;
      const listedFiles = await session.files().list();
      if (Array.isArray(listedFiles)) files = listedFiles;
    } catch {
      events = fallbackEvents;
      files = fallbackFiles;
    }

    const downloaded = [];
    for (const out of files) {
      let downloadedLen = 0;
      let sample = "";
      try {
        const bytes = await session.files().download(out);
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
    const sessionTerminalNames = new Set(["aex.session.idle", "aex.session.suspended", "aex.session.succeeded", "aex.session.failed", "aex.session.timed_out", "aex.session.cancelled"]);
    const isSessionIdle = (e) => e && e.type === "CUSTOM" && e.data && sessionTerminalNames.has(e.data.name);
    const terminal = events.find((e) => (e.type === "TURN_FINISHED" || e.type === "TURN_ERROR")) ?? events.find(isSessionIdle);
    const eventKinds = events.map((e) => e.type);
    if (terminal && isSessionIdle(terminal) && !eventKinds.includes("TURN_FINISHED")) eventKinds.push("TURN_FINISHED");
    const terminalData = terminal && isSessionIdle(terminal)
      ? { ...terminal.data.value, reason: terminal.data.value?.reason === "completed" ? "complete" : terminal.data.value?.reason }
      : terminal ? terminal.data : null;
    const streamErrors = events
      .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error")
      .map((e) => (e.data && typeof e.data === "object" ? e.data : { unknown: true }));

    const result = {
      sessionId: sessionId,
      sessionStatus: sessionInfo.status,
      runtime: sessionInfo.runtime ?? "(missing)",
      provider: sessionInfo.provider ?? "(missing)",
      marker: ${JSON.stringify(marker)},
      eventCount: events.length,
      eventKinds,
      files: downloaded,
      assistantTextJoined,
      terminalKind: terminal && isSessionIdle(terminal) ? "TURN_FINISHED" : terminal ? terminal.type : null,
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
  lines.push(`sessionStatus=${result.sessionStatus} runtime=${result.runtime} provider=${result.provider}`);
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
    AEX_API_KEY: apiKey,
    [cell.keyEnvName]: cell.keyValue
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
    let corruptSkillName = null;
    try {
      const hashBuf = await crypto.subtle.digest("SHA-256", corruptedZip);
      const hashHex = Array.from(new Uint8Array(hashBuf)).map((b) => b.toString(16).padStart(2, "0")).join("");
      const contentHash = "sha256:" + hashHex;

      const presignRes = await fetch(process.env.AEX_API_URL + "/assets/presign", {
        method: "POST",
        headers: {
          "content-type": "application/json",
          authorization: "Bearer " + process.env.AEX_API_KEY
        },
        body: JSON.stringify({ hash: contentHash, sizeBytes: corruptedZip.byteLength })
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
          const finalizeRes = await fetch(process.env.AEX_API_URL + "/assets/finalize", {
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
          const skillName = "corrupt-skill";
          const upsertRes = await fetch(process.env.AEX_API_URL + "/api/skills/" + skillName, {
            method: "PUT",
            headers: {
              "content-type": "application/json",
              authorization: "Bearer " + process.env.AEX_API_KEY
            },
            body: JSON.stringify({
              contentHash,
              description: "Corrupted skill bundle probe.",
              sizeBytes: corruptedZip.byteLength
            })
          });
          const upsertText = await upsertRes.text();
          submitStatus = upsertRes.status;
          submitBody = upsertText.slice(0, 800);
          if (upsertRes.ok) {
            corruptSkillName = skillName;
          } else {
            errorClass = "skill-upsert-rejected";
            errorMessage = "skill upsert rejected at status " + upsertRes.status + ": " + submitBody.slice(0, 200);
          }
        }
      }
    } catch (e) {
      errorClass = e && e.constructor ? e.constructor.name : "Error";
      errorCode = e && typeof e.code === "string" ? e.code : null;
      errorMessage = e && e.message ? e.message : String(e);
    }

    if (corruptSkillName) {
      try {
        // First-class skills ride submission.skills by name. The BFF resolves the
        // registry entry to boot-record-only asset metadata, then materialization
        // rejects the malformed bundle at submit or session time.
        const res = await fetch(process.env.AEX_API_URL + "/api/sessions", {
          method: "POST",
          headers: {
            "content-type": "application/json",
            authorization: "Bearer " + process.env.AEX_API_KEY
          },
          body: JSON.stringify({
            workspaceId: "ws-test",
            idempotencyKey: "fail-corrupt-skill-" + Date.now(),
            provider: "deepseek",
            submission: {
              model: ${JSON.stringify(deepseekModel)},
              prompt: ["Hello."],
              tools: [],
              skills: [{ kind: "skill", name: corruptSkillName }],
              agentsMd: [],
              files: [],
              mcpServers: []
            },
            secrets: { apiKeys: { deepseek: process.env.DEEPSEEK_KEY_SUBMIT } }
          })
        });
        submitStatus = res.status;
        submitBody = (await res.text()).slice(0, 800);
        submitOk = res.status >= 200 && res.status < 300;
        if (!submitOk) {
          errorClass = "session-submit-rejected";
          errorMessage = "corrupted skill bundle rejected at status " + res.status + ": " + submitBody.slice(0, 200);
        } else {
          try {
            const accepted = JSON.parse(submitBody);
            sessionId = typeof accepted.sessionId === "string"
              ? accepted.sessionId
              : typeof accepted.id === "string"
                ? accepted.id
                : null;
          } catch {
            sessionId = null;
          }
          if (!sessionId) {
            errorClass = "session-submit-missing-id";
            errorMessage = "corrupted skill bundle submit was accepted but returned no session id: " + submitBody.slice(0, 200);
          }
        }
      } catch (e) {
        errorClass = e && e.constructor ? e.constructor.name : "Error";
        errorCode = e && typeof e.code === "string" ? e.code : null;
        errorMessage = e && e.message ? e.message : String(e);
      }
    }

    if (submitOk && sessionId) {
      const terminalStatuses = new Set(["succeeded", "failed", "cancelled", "canceled", "timed_out", "expired", "deleted"]);
      const authHeaders = { authorization: "Bearer " + process.env.AEX_API_KEY };
      const deadline = Date.now() + Number(process.env.FAILURE_WAIT_MS || "120000");
      while (Date.now() < deadline) {
        try {
          sessionPollAttempts += 1;
          const sessionRes = await fetch(process.env.AEX_API_URL + "/api/sessions/" + encodeURIComponent(sessionId), {
            headers: authHeaders
          });
          sessionPollStatus = sessionRes.status;
          if (sessionRes.ok) {
            const sessionBody = await sessionRes.json();
            const sessionRecord = sessionBody && sessionBody.session && typeof sessionBody.session === "object" ? sessionBody.session : sessionBody;
            if (sessionRecord && typeof sessionRecord.status === "string") sessionStatus = sessionRecord.status;
            if (sessionRecord && typeof sessionRecord.errorMessage === "string") sessionErrorMessage = sessionRecord.errorMessage;
            if (sessionStatus && terminalStatuses.has(sessionStatus)) break;
          }
        } catch {
          // Keep polling until the failure contract either appears or times out.
        }
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }

      try {
        const eventsRes = await fetch(process.env.AEX_API_URL + "/api/sessions/" + encodeURIComponent(sessionId) + "/events?limit=1000", {
          headers: authHeaders
        });
        if (eventsRes.ok) {
          const eventsBody = await eventsRes.json();
          const events = Array.isArray(eventsBody.events) ? eventsBody.events : [];
          const terminal = events.find((event) => event && (event.type === "TURN_FINISHED" || event.type === "TURN_ERROR"));
          eventKinds = events.map((event) => event && typeof event.type === "string" ? event.type : "UNKNOWN");
          terminalKind = terminal && typeof terminal.type === "string" ? terminal.type : null;
          terminalData = terminal && terminal.data && typeof terminal.data === "object" ? terminal.data : null;
          streamErrors = events
            .filter((event) => event && event.type === "CUSTOM" && event.data && event.data.name === "aex.stream_error")
            .map((event) => event.data && typeof event.data.value === "object" ? event.data.value : event.data);
        }
      } catch {
        // The session record is authoritative for this assertion; events enrich the
        // failure contract when the stream endpoint is available.
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
        provider: "deepseek",
        runtimeSize: "native",
        model: "deepseek-v4-flash",
        message: "Hello.",
        apiKeys: { deepseek: process.env.DEEPSEEK_KEY_SUBMIT ?? "sk-test" },
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
          authorization: "Bearer " + process.env.AEX_API_KEY
        },
        body: JSON.stringify({
          workspaceId: "ws-test",
          idempotencyKey: "fail-stdio-mcp-" + Date.now(),
          provider: "deepseek",
          submission: {
            model: ${JSON.stringify(deepseekModel)},
            prompt: ["Hello."],
            agentsMd: [],
            files: [],
            mcpServers: [{ name: "bad-stdio", url: "stdio:///dev/null", transport: "stdio" }]
          },
          secrets: { apiKeys: { deepseek: process.env.DEEPSEEK_KEY_SUBMIT } }
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
    AEX_API_KEY: apiKey,
    DEEPSEEK_KEY_SUBMIT: deepseekKey
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
  it.each(CELLS)(
    "$id: agent writes report.txt with marker, listFiles + download recovers it",
    async (cell) => {
      const result = await startFileCell(cell, install.installDir);
      const dump = (): string => dumpFileResult(cell, result);

      expect(result.sessionStatus, dump()).toBe("succeeded");
      expect(result.runtime).toBe("managed");
      expect(result.provider).toBe(cell.provider);
      expect(result.terminalKind).toBe("TURN_FINISHED");
      // Every clean terminal MUST carry reason="complete" — both adapters
      // always populate reason on the success path. Tolerating `undefined`
      // (pre-Phase-1) was masking field-loss regressions.
      const terminalReason = result.terminalData ? result.terminalData["reason"] : undefined;
      if (terminalReason !== "complete") {
        throw new Error(`terminal reason=${terminalReason} (expected "complete")\n\n${dump()}`);
      }
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
    "b1 corrupted skill zip: 4xx at submit OR session status:failed with structured error",
    async () => {
      const result = await runFailureCase(
        buildCorruptedSkillScript,
        "fail-corrupted-skill.mjs",
        install.installDir
      );
      const dump = (): string => dumpFailureResult(result);

      // Accept either branch:
      //  - submit threw with a structured AexError (errorClass non-null)
      //  - submit succeeded but the session reached terminal "failed" with
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
        // submit accepted; the session must have failed terminally with a
        // reported error. The structured failure can surface as
        // SessionRecord.errorMessage OR terminalData.failureMessage — both are
        // legitimate places the API plumbs the cause; require at least
        // one to be populated so a silent failure is caught.
        if (result.sessionStatus !== "failed") {
          throw new Error(
            `submit accepted but session did not fail (status=${result.sessionStatus})\n\n${dump()}`
          );
        }
        const terminalFailureMsg =
          result.terminalData && typeof result.terminalData["failureMessage"] === "string"
            ? (result.terminalData["failureMessage"] as string)
            : "";
        const haveStructuredCause =
          (result.sessionErrorMessage && result.sessionErrorMessage.length > 0) ||
          terminalFailureMsg.length > 0;
        if (!haveStructuredCause) {
          throw new Error(
            `session failed but no errorMessage on SessionRecord AND no terminalData.failureMessage\n\n${dump()}`
          );
        }
        // A runtime_terminal with reason!="complete" is the on-stream
        // signal that the session did not finish cleanly. Either reason ===
        // "error" OR a stream_error event must be present.
        const terminalReason = result.terminalData ? result.terminalData["reason"] : undefined;
        const sawSignal =
          (typeof terminalReason === "string" && terminalReason !== "complete") ||
          result.streamErrors.length > 0;
        if (!sawSignal) {
          throw new Error(
            `session failed but no terminal reason!=complete and no stream_error events\n\n${dump()}`
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
