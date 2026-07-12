/**
 * Live user tests for post-RUN consistency and session-list pagination.
 *
 * These tests use only the public installed SDK. A committed RUN terminal is
 * the consistency barrier: no post-finish polling or scenario retry is allowed.
 */
import { randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  buildEdgeListSearchChildScript,
  EDGE_SESSION_DEBUG_BODY
} from "../_fixtures/edge-list-search-child.js";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`edge-list-search: required env ${name} is missing`);
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const providerKey = requireGateKey("edge-list-search");
const model = gateModel();

function childEnv(): Record<string, string> {
  const env: Record<string, string> = {
    AEX_API_URL: apiUrl,
    AEX_API_KEY: apiKey,
    PROVIDER: GATE_PROVIDER,
    PROVIDER_KEY: providerKey,
    MODEL: model
  };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  const carry = process.platform === "win32"
    ? ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]
    : ["HOME", "TMPDIR", "LANG", "LC_ALL"];
  for (const key of carry) if (process.env[key]) env[key] = process.env[key]!;
  return env;
}

async function runChild(
  install: InstallResult,
  scriptName: string,
  body: string,
  timeoutMs = 10 * 60_000
): Promise<Record<string, unknown>> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, buildEdgeListSearchChildScript(body));
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: childEnv()
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-list-search child ${scriptName} exited ${child.exitCode}:\n` +
      `--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => install?.cleanup());

describe("edge: finished consistency and session listing", () => {
  it("RUN_FINISHED makes session state, checkpoint files, billing, and unit reads immediately consistent", async () => {
    const marker = `finish-consistent-${randomBytes(6).toString("hex")}.txt`;
    const out = await runChild(install, "edge-finish-consistency.mjs", `
      const result = await client.start({
        provider: PROVIDER,
        model: MODEL,
        message: ${JSON.stringify(`Use bash to write exactly "finish-consistent" to /workspace/files/${marker}, then reply done.`)},
        builtinTools: "default",
        fileCapture: { allowedDirs: ["/workspace/files"] },
        apiKeys: { [PROVIDER]: PROVIDER_KEY },
        idempotencyKey: "edge-finish-consistency-" + Date.now()
      }, { timeoutMs: 6 * 60_000 });

      const terminals = result.events.filter((event) => event.type === "RUN_FINISHED" || event.type === "RUN_ERROR");
      if (terminals.length !== 1) throw new Error("expected exactly one RUN terminal");
      const terminal = terminals[0];
      const session = await client.sessions.open(result.sessionId);
      const snapshot = await session.files.list();
      const serialized = JSON.stringify({ result, record: session.record, snapshot });

      process.stdout.write(JSON.stringify({
        sessionId: result.sessionId,
        resultStatus: result.status,
        resultOk: result.ok,
        resultCheckpointId: result.checkpoint?.checkpointId ?? null,
        resultCostUsd: result.costUsd,
        resultUsageIsObject: !!result.usage && typeof result.usage === "object",
        terminalType: terminal.type,
        terminalOutcome: terminal.data?.outcome ?? null,
        terminalCostUsd: terminal.data?.costUsd ?? null,
        terminalProviderUsageIsArray: Array.isArray(terminal.data?.providerUsage),
        terminalCheckpointId: terminal.data?.checkpoint?.checkpointId ?? null,
        sessionStatus: session.record.status,
        acceptsMessages: session.record.acceptsMessages,
        lastRunOutcome: session.record.lastRun?.outcome ?? null,
        snapshotCheckpointId: snapshot.revision.checkpointId,
        snapshotRunId: snapshot.revision.runId,
        fileNames: snapshot.files.map((file) => file.filename),
        leakedApiKey: serialized.includes(process.env.AEX_API_KEY),
        leakedProviderKey: serialized.includes(PROVIDER_KEY)
      }));
    `);

    const dump = JSON.stringify(out, null, 2);
    expect(out.resultOk, dump).toBe(true);
    expect(out.resultStatus, dump).toBe("succeeded");
    expect(out.terminalType, dump).toBe("RUN_FINISHED");
    expect(out.terminalOutcome, dump).toBe("succeeded");
    expect(out.terminalCostUsd, dump).toBeTypeOf("number");
    expect(out.terminalProviderUsageIsArray, dump).toBe(true);
    expect(out.terminalCheckpointId, dump).toBe(out.resultCheckpointId);
    expect(out.snapshotCheckpointId, dump).toBe(out.resultCheckpointId);
    expect(out.sessionStatus, dump).toBe("idle");
    expect(out.acceptsMessages, dump).toBe(true);
    expect(out.lastRunOutcome, dump).toBe("succeeded");
    expect(out.fileNames, dump).toContain(marker);
    expect(out.resultCostUsd, dump).toBeTypeOf("number");
    expect(out.resultUsageIsObject, dump).toBe(true);
    expect(out.leakedApiKey, dump).toBe(false);
    expect(out.leakedProviderKey, dump).toBe(false);
  }, 12 * 60_000);

  it("sessions.list paginates without duplicates and exposes only lifecycle statuses", async () => {
    const out = await runChild(install, "edge-session-pagination.mjs", `
      const created = [];
      const since = new Date(Date.now() - 60_000).toISOString();
      try {
        for (let i = 0; i < 3; i++) {
          created.push(await client.sessions.create({
            provider: PROVIDER,
            model: MODEL,
            apiKeys: { [PROVIDER]: PROVIDER_KEY },
            idempotencyKey: "edge-pagination-" + Date.now() + "-" + i
          }));
        }

        const ids = [];
        const statuses = [];
        const cursors = new Set();
        let cursor;
        let pages = 0;
        do {
          if (cursor !== undefined) {
            if (cursors.has(cursor)) throw new Error("sessions.list repeated a cursor");
            cursors.add(cursor);
          }
          const page = await client.sessions.list({ since, limit: 2, ...(cursor ? { cursor } : {}) });
          if (page.sessions.length > 2) throw new Error("sessions.list exceeded its requested limit");
          ids.push(...page.sessions.map((session) => session.id));
          statuses.push(...page.sessions.map((session) => session.status));
          cursor = page.nextCursor;
          pages++;
          if (pages > 1000) throw new Error("sessions.list did not terminate");
        } while (cursor);

        const future = await client.sessions.list({ since: new Date(Date.now() + 60_000).toISOString(), limit: 10 });
        const invalid = [];
        for (const query of [{ limit: 0 }, { limit: 101 }, { limit: 1.5 }, { status: "succeeded" }, { since: "yesterday" }]) {
          try {
            await client.sessions.list(query);
            invalid.push("resolved");
          } catch (error) {
            invalid.push(error?.name ?? "Error");
          }
        }

        process.stdout.write(JSON.stringify({
          createdIds: created.map((session) => session.id),
          ids,
          statuses,
          pages,
          futureCount: future.sessions.length,
          invalid
        }));
      } finally {
        await Promise.all(created.map((session) => session.delete()));
      }
    `);

    const dump = JSON.stringify(out, null, 2);
    const createdIds = out.createdIds as string[];
    const ids = out.ids as string[];
    const statuses = out.statuses as string[];
    const lifecycle = new Set([
      "creating", "running", "idle", "suspending", "suspended", "awaiting_approval",
      "error", "cancelling", "deleting", "deleted", "expired"
    ]);
    expect(createdIds.every((id) => ids.includes(id)), dump).toBe(true);
    expect(new Set(ids).size, dump).toBe(ids.length);
    expect(statuses.every((status) => lifecycle.has(status)), dump).toBe(true);
    expect(out.futureCount, dump).toBe(0);
    expect(out.invalid, dump).toEqual([
      "SessionConfigValidationError",
      "SessionConfigValidationError",
      "SessionConfigValidationError",
      "SessionConfigValidationError",
      "SessionConfigValidationError"
    ]);
  }, 10 * 60_000);

  it("debug output is emitted through the configured sink without leaking credentials", async () => {
    const out = await runChild(install, "edge-session-debug.mjs", EDGE_SESSION_DEBUG_BODY, 60_000);

    expect(out.count).toBeGreaterThan(0);
    expect(out.leakedApiKey).toBe(false);
    expect(out.leakedProviderKey).toBe(false);
  }, 180_000);
});
