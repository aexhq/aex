/**
 * Live scenario: live-sdk-deepseek.test.ts
 *
 * Drives the **published `@aexhq/sdk` SDK** against the live
 * api.aex.dev hosted API with a real DeepSeek round-trip on the
 * managed runtime: SDK → /api/sessions → control-plane workflow → managed
 * runtime → real managed-runtime process → BYOK provider-proxy → api.deepseek.com →
 * stream-json events → terminal. No smoke shortcut.
 *
 * This is the canonical "customer uses the SDK to run a minimal
 * DeepSeek agent (no skills, no MCP, no AGENTS.md)" path.
 * `live-sdk-comprehensive.test.ts` covers the full feature surface
 * for the same (provider, runtime) cell.
 *
 * Required env:
 *   AEX_API_URL              live api.aex.dev URL
 *   DEEPSEEK_API_KEY     customer's DeepSeek API key
 *   AEX_USER_TEST_TARBALL          path to a packed aex tgz
 *     OR AEX_USER_TEST_VERSION     published package version
 *
 * Optional:
 *   AEX_USER_TEST_DEEPSEEK_MODEL            default "deepseek-v4-flash"
 *
 * TODO (D7 cost tracking, not yet implemented): when enabled, an
 * AEX_COST_LOG_PATH env var would have these tests append a JSONL line per
 * LLM round-trip ({ ts, test, provider, model, promptTokens,
 * completionTokens, estimatedUsd }) consumed by
 * scripts/cicd/smoke-cost-budget.mjs. The tests do NOT read or write this
 * var today — it is not an active env contract.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(
      `user-tests live: required env ${name} is missing. The live DeepSeek scenario must run against a real api.aex.dev URL with a real DeepSeek key.`
    );
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const model = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

interface LiveResult {
  readonly sessionId: string;
  readonly runStatus: string;
  readonly probe: string;
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly assistantTextJoined: string;
  readonly assistantTextEventCount: number;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly fileCount: number;
  // Per-file filenames + sizes returned by GET /api/sessions/:id/files.
  // Captured for diagnostic dumps so files-surface failures are
  // self-describing without a retry. Namespace separation is covered by
  // live-sdk-download-namespaces.test.ts; this simple text round-trip does not
  // assert that the model/runtime produced no user deliverables.
  readonly files: ReadonlyArray<{ readonly filename: string; readonly sizeBytes: number }>;
  readonly leakedDeepseekKey: boolean;
}

describe("live api.aex.dev via installed SDK — DeepSeek round-trip on managed runtime", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "submits via SDK, waits for terminal, fetches events + files, asserts a real DeepSeek response landed in the event log",
    async () => {
      // Drive the SDK from a child Bun process whose cwd is the
      // install tempdir, so `import "aex"` resolves to the
      // installed tarball — not the monorepo workspace symlink.
      const probe = "e2e-marker-" + Math.random().toString(36).slice(2, 8);
      const script = `
        import { Aex } from "@aexhq/sdk";

        const apiBase = process.env.AEX_API_URL;
        const deepseekKey = process.env.DEEPSEEK_KEY;
        const model = process.env.MODEL;
        const apiKey = process.env.AEX_API_KEY;

        // Phase 7 wired workspace-token auth on POST /api/sessions; the apiKey
        // is now a real, workspace-scoped credential. The live runner
        // gets it via env from the spawning test.
        const client = new Aex({
          baseUrl: apiBase,
          apiKey
        });

        // Run a DeepSeek agent. The SDK's start() opens a one-shot session,
        // streams through the durable run terminal, and returns the collected SessionResult. Real
        // managed-runtime sessions take a while (cold-start + image pull +
        // process startup + LLM round-trip), so give it up to 8 minutes.
        const sessionResult = await client.start({
          model,
          message: ${JSON.stringify(`Reply with exactly the following token and nothing else, character for character: ${probe}`)},
          idempotencyKey: "user-test-deepseek-" + Date.now(),
        }, { timeoutMs: 8 * 60 * 1000 });
        const sessionId = sessionResult.sessionId;
        // stderr, not stdout: the parent parses stdout as one JSON blob. The
        // sessionId must land in the CI log even if this script dies before emit.
        process.stderr.write("sessionId=" + sessionId + "\\n");
        const run = {
          status: sessionResult.status,
          runtime: "managed",
        };
        const session = await client.sessions.open(sessionId);
        const events = (await session.events.list()).filter((event) => event.runId === sessionResult.run.runId);
        const files = (await session.files.list()).files;
        const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
        const assistantTextJoined = assistantTextEvents
          .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
          .join(" ");
        const terminal = events.find((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR");
        const eventKinds = events.map((e) => e.type);
        const terminalData = terminal ? terminal.data : null;

        const serialized = JSON.stringify({ run, events, files });
        const result = {
          sessionId: sessionId,
          runStatus: run.status,
          probe: ${JSON.stringify(probe)},
          eventCount: events.length,
          eventKinds,
          assistantTextJoined,
          assistantTextEventCount: assistantTextEvents.length,
          terminalKind: terminal ? terminal.type : null,
          terminalData,
          fileCount: files.length,
          files: files.map((o) => ({ filename: o.filename, sizeBytes: o.sizeBytes })),
          leakedDeepseekKey: serialized.includes(deepseekKey)
        };
        process.stdout.write(JSON.stringify(result));
        process.exit(0);
      `;
      const scriptPath = join(install.installDir, "live-deepseek-runner.mjs");
      writeFileSync(scriptPath, script);

      // Sanitize the child env — pass only what the SDK consumer needs,
      // so a regression that depends on a CI-only secret can't pass
      // silently.
      const apiKey = requireEnv("AEX_API_KEY");
      const passEnv: Record<string, string> = {
        AEX_API_URL: apiUrl,
        AEX_API_KEY: apiKey,
        DEEPSEEK_KEY: deepseekKey,
        MODEL: model
      };
      const pathKey = process.platform === "win32" ? "Path" : "PATH";
      if (process.env[pathKey]) passEnv[pathKey] = process.env[pathKey]!;
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
          if (process.env[k]) passEnv[k] = process.env[k]!;
        }
      } else {
        for (const k of ["HOME", "TMPDIR", "LANG", "LC_ALL"]) {
          if (process.env[k]) passEnv[k] = process.env[k]!;
        }
      }

      const child = await runCommand(getBunCommand(), [scriptPath], {
        cwd: install.installDir,
        timeoutMs: 10 * 60 * 1000,
        env: passEnv
      });
      if (child.exitCode !== 0) {
        throw new Error(
          `live runner exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
        );
      }

      const result = JSON.parse(child.stdout.trim()) as LiveResult;

      // Print the sessionId as soon as we have it so the platform diagnostics
      // collector can pull this session's forensics from the CI log even when a
      // later assertion fails (same convention as live-sdk-heavy-session).
      console.log(`sessionId=${result.sessionId} runStatus=${result.runStatus} terminalKind=${result.terminalKind}`);

      // ---- assertions ----

      const failureClass =
        result.terminalData && typeof result.terminalData["failureClass"] === "string"
          ? (result.terminalData["failureClass"] as string)
          : null;
      expect(
        result.runStatus,
        `sessionId=${result.sessionId} terminalKind=${result.terminalKind} failureClass=${failureClass} terminalData=${JSON.stringify(result.terminalData)}`
      ).toBe("succeeded");

      // The committed public terminal and assistant text prove the managed run
      // completed through the installed SDK.
      expect(result.terminalKind).toBe("RUN_FINISHED");
      expect(result.eventKinds).toContain("RUN_FINISHED");
      expect(result.assistantTextEventCount).toBeGreaterThan(0);
      expect(result.assistantTextJoined.length).toBeGreaterThan(0);
      // The managed runtime stream fragments responses across content blocks
      // (each assistant_text event may carry a fragment of a single
      // tokenised word — observed in production: "e 2 e -m arker -h ng
      // m cg" instead of "e2e-marker-hngmcg"). Strip whitespace before
      // checking probe presence so the test is robust to tokenisation.
      const normalized = result.assistantTextJoined.replace(/\s+/g, "");
      expect(normalized).toContain(result.probe);

      const terminal = result.terminalData ?? {};
      expect(terminal["outcome"]).toBe("succeeded");

      // The customer's DeepSeek key MUST NOT appear anywhere in the
      // SDK-visible response surface.
      expect(result.leakedDeepseekKey).toBe(false);
    },
    11 * 60 * 1000
  );
});
