/**
 * Live installed-CLI coverage.
 *
 * This is the user-facing blackbox path for the README's `aex start --follow`
 * example. It installs the packed/published SDK artifact into a clean tempdir,
 * drives the installed `aex` binary against the live public API, then exercises
 * read/download host verbs against the same run.
 *
 * Required env:
 *   AEX_API_URL
 *   AEX_API_KEY
 *   DEEPSEEK_API_KEY
 *   AEX_USER_TEST_TARBALL | AEX_USER_TEST_VERSION (optional; otherwise local pack)
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { unzipSync } from "fflate";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getAexBinPath, installAex, runCommand, type InstallResult, type SessionResult } from "../_fixtures/install.js";

interface LiveCliEnv {
  readonly apiBase: string;
  readonly apiKey: string;
  readonly deepseekKey: string;
  readonly deepseekModel: string;
}

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`live-cli-installed: required env ${name} is missing`);
  }
  return value;
}

function requireLiveCliEnv(): LiveCliEnv {
  const apiBase = requireEnv("AEX_API_URL").replace(/\/+$/, "");
  if (!/^https?:\/\//.test(apiBase)) {
    throw new Error("live-cli-installed: AEX_API_URL must be an absolute http(s) URL");
  }
  return {
    apiBase,
    apiKey: requireEnv("AEX_API_KEY"),
    deepseekKey: requireEnv("DEEPSEEK_API_KEY"),
    deepseekModel: process.env.AEX_USER_TEST_DEEPSEEK_MODEL?.trim() || "deepseek-v4-flash"
  };
}

const env = requireLiveCliEnv();

// RUN_FINISHED is a consistency barrier: the session is immediately idle and
// ready for another message on every subsequent read.
const SESSION_READY = ["idle"];

function redactSecrets(text: string): string {
  return text.split(env.apiKey).join("[REDACTED_AEX_API_KEY]").split(env.deepseekKey).join("[REDACTED_DEEPSEEK_API_KEY]");
}

function commandDiagnostic(command: string, result: SessionResult): string {
  return redactSecrets(
    `${command} exited ${result.exitCode}\n--- stdout ---\n${result.stdout}\n--- stderr ---\n${result.stderr}`
  );
}

function parseJsonLines(stdout: string): Record<string, unknown>[] {
  return stdout
    .trim()
    .split(/\r?\n/)
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>);
}

function eventText(events: readonly Record<string, unknown>[]): string {
  return events
    .filter((event) => event["type"] === "TEXT_MESSAGE_CONTENT")
    .map((event) => {
      const data = event["data"];
      if (!data || typeof data !== "object" || Array.isArray(data)) return "";
      const text = (data as Record<string, unknown>)["text"];
      return typeof text === "string" ? text : "";
    })
    .join("");
}

function hasCleanTerminal(events: readonly Record<string, unknown>[]): boolean {
  const kinds = events.map((event) => event["type"]);
  return kinds.includes("RUN_FINISHED");
}

describe("live hosted API via installed CLI", () => {
  let install: InstallResult;
  let binPath: string;

  beforeAll(async () => {
    install = await installAex();
    binPath = getAexBinPath(install.installDir);
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  async function executeCli(args: readonly string[], timeoutMs = 60_000): Promise<SessionResult> {
    return await runCommand(binPath, args, {
      cwd: install.installDir,
      timeoutMs
    });
  }

  function commonArgs(): string[] {
    return ["--api-key", env.apiKey, "--aex-url", env.apiBase];
  }

  async function runCliCreate(args: readonly string[], timeoutMs: number): Promise<SessionResult> {
    return executeCli(args, timeoutMs);
  }

  it("submits with start --follow, then reads status/events/files/wait/download through the installed binary", async () => {
      const marker = `CLI-LIVE-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
      const run = await runCliCreate(
        [
          "start",
          "--provider",
          "deepseek",
          "--model",
          env.deepseekModel,
          "--prompt",
          `Return exactly this token as visible assistant text and no other words: ${marker}`,
          "--deepseek-api-key",
          env.deepseekKey,
          "--idempotency-key",
          `live-cli-installed-${marker.toLowerCase()}`,
          "--follow",
          "--timeout",
          "8m",
          ...commonArgs()
        ],
        10 * 60_000
      );
      const runDiag = commandDiagnostic("aex start --follow", run);
      expect(run.exitCode, runDiag).toBe(0);

      const runLines = parseJsonLines(run.stdout);
      const initial = runLines[0]!;
      const sessionId = initial["id"];
      expect(typeof sessionId, runDiag).toBe("string");
      const finalFromFollow = [...runLines].reverse().find((line) => line["id"] === sessionId && typeof line["status"] === "string");
      expect(SESSION_READY, runDiag).toContain(String(finalFromFollow?.["status"]));

      const status = await executeCli(["status", sessionId as string, ...commonArgs()]);
      expect(status.exitCode, commandDiagnostic("aex status", status)).toBe(0);
      const statusDoc = JSON.parse(status.stdout.trim()) as Record<string, unknown>;
      expect(statusDoc["id"], commandDiagnostic("aex status", status)).toBe(sessionId);
      expect(SESSION_READY, commandDiagnostic("aex status", status)).toContain(String(statusDoc["status"]));

      const wait = await executeCli(["wait", sessionId as string, "--timeout", "1m", "--interval", "1s", ...commonArgs()], 90_000);
      expect(wait.exitCode, commandDiagnostic("aex wait", wait)).toBe(0);
      const waitDoc = JSON.parse(wait.stdout.trim()) as Record<string, unknown>;
      expect(waitDoc["id"], commandDiagnostic("aex wait", wait)).toBe(sessionId);
      expect(SESSION_READY, commandDiagnostic("aex wait", wait)).toContain(String(waitDoc["status"]));

      const events = await executeCli(["events", sessionId as string, ...commonArgs()]);
      expect(events.exitCode, commandDiagnostic("aex events", events)).toBe(0);
      const eventRows = parseJsonLines(events.stdout);
      const eventKinds = eventRows.map((event) => event["type"]);
      expect(eventKinds, commandDiagnostic("aex events", events)).toContain("RUN_STARTED");
      expect(hasCleanTerminal(eventRows), commandDiagnostic("aex events", events)).toBe(true);
      const visibleText = eventText(eventRows).replace(/\s+/g, "");
      expect(visibleText, commandDiagnostic("aex events", events)).toContain(marker);

      const files = await executeCli(["files", sessionId as string, ...commonArgs()]);
      expect(files.exitCode, commandDiagnostic("aex files", files)).toBe(0);
      const outputRows = files.stdout.trim().length > 0 ? parseJsonLines(files.stdout) : [];
      for (const output of outputRows) {
        expect(typeof output["id"], commandDiagnostic("aex files", files)).toBe("string");
      }

      const archivePath = join(install.installDir, `live-cli-events-${sessionId}.zip`);
      const download = await executeCli(["download", sessionId as string, "--only", "events", "--out", archivePath, ...commonArgs()]);
      expect(download.exitCode, commandDiagnostic("aex download --only events", download)).toBe(0);
      expect(JSON.parse(download.stdout.trim())).toMatchObject({
        sessionId,
        namespace: "events",
        path: archivePath
      });
      expect(existsSync(archivePath)).toBe(true);
      const entries = unzipSync(new Uint8Array(readFileSync(archivePath)));
      expect(Object.keys(entries).sort()).toEqual(["events.jsonl", "manifest.json"]);
      const archivedEvents = parseJsonLines(new TextDecoder().decode(entries["events.jsonl"]!));
      expect(hasCleanTerminal(archivedEvents)).toBe(true);
  }, 35 * 60_000);
});
