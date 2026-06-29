/**
 * Live installed-CLI coverage.
 *
 * This is the user-facing blackbox path for the README's `aex run --follow`
 * example. It installs the packed/published SDK artifact into a clean tempdir,
 * drives the installed `aex` binary against the live public API, then exercises
 * read/download host verbs against the same run.
 *
 * Required env:
 *   AEX_API_URL
 *   AEX_API_TOKEN
 *   DEEPSEEK_API_KEY
 *   AEX_USER_TEST_TARBALL | AEX_USER_TEST_VERSION (optional; otherwise local pack)
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { unzipSync } from "fflate";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getAexBinPath, installAex, runCommand, type InstallResult, type RunResult } from "../_fixtures/install.js";

interface LiveCliEnv {
  readonly apiBase: string;
  readonly apiToken: string;
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
    apiToken: requireEnv("AEX_API_TOKEN"),
    deepseekKey: requireEnv("DEEPSEEK_API_KEY"),
    deepseekModel: process.env.AEX_USER_TEST_DEEPSEEK_MODEL ?? "deepseek-chat"
  };
}

const env = requireLiveCliEnv();

function redactSecrets(text: string): string {
  return text.split(env.apiToken).join("[REDACTED_AEX_API_TOKEN]").split(env.deepseekKey).join("[REDACTED_DEEPSEEK_API_KEY]");
}

function commandDiagnostic(command: string, result: RunResult): string {
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

  async function runCli(args: readonly string[], timeoutMs = 60_000): Promise<RunResult> {
    return await runCommand(binPath, args, {
      cwd: install.installDir,
      timeoutMs
    });
  }

  function commonArgs(): string[] {
    return ["--api-token", env.apiToken, "--aex-url", env.apiBase];
  }

  it("submits with run --follow, then reads status/events/outputs/wait/download through the installed binary", async () => {
    const marker = `CLI-LIVE-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    const run = await runCli(
      [
        "run",
        "--provider",
        "deepseek",
        "--model",
        env.deepseekModel,
        "--prompt",
        `Reply with exactly this token and no other words: ${marker}`,
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
    expect(run.exitCode, commandDiagnostic("aex run --follow", run)).toBe(0);

    const runLines = parseJsonLines(run.stdout);
    const initial = runLines[0]!;
    const runId = initial["id"];
    expect(typeof runId, commandDiagnostic("aex run --follow", run)).toBe("string");
    const finalFromFollow = [...runLines].reverse().find((line) => line["id"] === runId && typeof line["status"] === "string");
    expect(finalFromFollow?.["status"], commandDiagnostic("aex run --follow", run)).toBe("succeeded");

    const status = await runCli(["status", runId as string, ...commonArgs()]);
    expect(status.exitCode, commandDiagnostic("aex status", status)).toBe(0);
    expect(JSON.parse(status.stdout.trim())).toMatchObject({ id: runId, status: "succeeded" });

    const wait = await runCli(["wait", runId as string, "--timeout", "1m", "--interval", "1s", ...commonArgs()], 90_000);
    expect(wait.exitCode, commandDiagnostic("aex wait", wait)).toBe(0);
    expect(JSON.parse(wait.stdout.trim())).toMatchObject({ id: runId, status: "succeeded" });

    const events = await runCli(["events", runId as string, ...commonArgs()]);
    expect(events.exitCode, commandDiagnostic("aex events", events)).toBe(0);
    const eventRows = parseJsonLines(events.stdout);
    const eventKinds = eventRows.map((event) => event["type"]);
    expect(eventKinds, commandDiagnostic("aex events", events)).toContain("RUN_STARTED");
    expect(eventKinds, commandDiagnostic("aex events", events)).toContain("RUN_FINISHED");
    expect(eventText(eventRows).replace(/\s+/g, ""), commandDiagnostic("aex events", events)).toContain(marker);

    const outputs = await runCli(["outputs", runId as string, ...commonArgs()]);
    expect(outputs.exitCode, commandDiagnostic("aex outputs", outputs)).toBe(0);
    const outputRows = outputs.stdout.trim().length > 0 ? parseJsonLines(outputs.stdout) : [];
    for (const output of outputRows) {
      expect(typeof output["id"], commandDiagnostic("aex outputs", outputs)).toBe("string");
    }

    const archivePath = join(install.installDir, `live-cli-events-${runId}.zip`);
    const download = await runCli(["download", runId as string, "--only", "events", "--out", archivePath, ...commonArgs()]);
    expect(download.exitCode, commandDiagnostic("aex download --only events", download)).toBe(0);
    expect(JSON.parse(download.stdout.trim())).toMatchObject({
      runId,
      namespace: "events",
      path: archivePath
    });
    expect(existsSync(archivePath)).toBe(true);
    const entries = unzipSync(new Uint8Array(readFileSync(archivePath)));
    expect(Object.keys(entries).sort()).toEqual(["events.jsonl"]);
    expect(new TextDecoder().decode(entries["events.jsonl"]!).trim()).toContain("RUN_FINISHED");
  }, 12 * 60_000);
});
