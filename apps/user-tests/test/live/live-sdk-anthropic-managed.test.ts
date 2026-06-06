/**
 * Live scenario: live-sdk-deepseek-managed-a.test.ts
 *
 * Sibling of live-sdk-deepseek.test.ts. Same managed runtime runtime
 * path, swapped provider — DeepSeek via the BYOK provider-proxy.
 *
 *   SDK → POST /api/runs { provider: "deepseek", runtime: "managed" }
 *      → hosted run-lifecycle → managed runtime
 *      → /provider-proxy/anthropic-messages/v1/messages
 *        (managed runtime dials ANTHROPIC_HOST; runtimeProvider="anthropic" → Anthropic
 *         shape, NOT the OpenAI-compat layer, which aex no longer
 *         serves for Anthropic — it loses prompt caching)
 *      → hosted API injects vault'd DeepSeek key
 *      → api.anthropic.com /v1/messages
 *      → real deepseek-chat response → assistant_text event
 *
 * The customer may omit runtime or pass runtime:'managed'; both use the
 * same managed sandbox semantics as every other provider. The dispatcher
 * rejects provider-hosted skill refs on managed, so this path uses local
 * SDK/object storage assets when skills or files are needed.
 *
 * Required env:
 *   AEX_API_URL              live api.aex.dev URL
 *   DEEPSEEK_API_KEY    customer's DeepSeek API key
 *   AEX_USER_TEST_TARBALL          path to packed aex tgz
 *     OR AEX_USER_TEST_VERSION     published version on npm
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(
      `user-tests live: required env ${name} is missing. The live DeepSeek-Managed scenario must run against a real api.aex.dev URL with a real DeepSeek key.`
    );
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const model = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"] ?? "deepseek-chat";

interface LiveResult {
  readonly runId: string;
  readonly runStatus: string;
  readonly runtime: string;
  readonly provider: string;
  readonly probe: string;
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly assistantTextJoined: string;
  readonly assistantTextEventCount: number;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly outputCount: number;
  // Per-file filenames + sizes returned by GET /api/runs/:id/outputs.
  // Captured for diagnostic dumps so a deliverable-related failure is
  // self-describing without a re-run. Namespace separation is covered by
  // live-sdk-download-namespaces.test.ts; this simple text round-trip does not
  // assert that the model/runtime produced no user deliverables.
  readonly outputs: ReadonlyArray<{ readonly filename: string; readonly sizeBytes: number }>;
  readonly leakedProviderKey: boolean;
}

describe("live api.aex.dev via installed SDK — DeepSeek round-trip on managed runtime runtime", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "submits via SDK with runtime:'managed', waits for terminal, asserts a real DeepSeek response landed in the event log",
    async () => {
      const probe = "e2e-marker-" + Math.random().toString(36).slice(2, 8);
      const script = `
        import { AgentExecutor } from "@aexhq/sdk";

        const apiBase = process.env.AEX_API_URL;
        const deepseekKey = process.env.DEEPSEEK_KEY;
        const model = process.env.MODEL;
        const apiToken = process.env.AEX_API_TOKEN;

        const client = new AgentExecutor({
          baseUrl: apiBase,
          apiToken
        });

        const runId = await client.submitRun({
          provider: "deepseek",
          runtime: "managed",
          model,
          prompt: ${JSON.stringify(`Output verbatim: ${probe}`)},
          idempotencyKey: "user-test-deepseek-mgd-" + Date.now(),
          secrets: { deepseek: { apiKey: deepseekKey } }
        });

        const deadline = Date.now() + 8 * 60 * 1000;
        let run = null;
        while (Date.now() < deadline) {
          run = await client.getRun(runId);
          if (run.status === "succeeded" || run.status === "failed" || run.status === "cancelled") {
            break;
          }
          await new Promise((r) => setTimeout(r, 3_000));
        }
        if (!run || (run.status !== "succeeded" && run.status !== "failed" && run.status !== "cancelled")) {
          process.stderr.write(JSON.stringify({ kind: "timeout", run }, null, 2));
          process.exit(2);
        }

        const events = await client.listEvents(runId);
        const outputs = await client.listOutputs(runId);
        const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
        const assistantTextJoined = assistantTextEvents
          .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
          .join(" ");
        const terminal = events.find((e) => (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR"));

        const serialized = JSON.stringify({ run, events, outputs });
        const result = {
          runId: runId,
          runStatus: run.status,
          runtime: run.runtime ?? "(missing)",
          provider: run.provider ?? "(missing)",
          probe: ${JSON.stringify(probe)},
          eventCount: events.length,
          eventKinds: events.map((e) => e.type),
          assistantTextJoined,
          assistantTextEventCount: assistantTextEvents.length,
          terminalKind: terminal ? terminal.type : null,
          terminalData: terminal ? terminal.data : null,
          outputCount: outputs.length,
          outputs: outputs.map((o) => ({ filename: o.filename, sizeBytes: o.sizeBytes })),
          leakedProviderKey: serialized.includes(deepseekKey)
        };
        process.stdout.write(JSON.stringify(result));
      `;
      const scriptPath = join(install.installDir, "live-deepseek-managed-a-runner.mjs");
      writeFileSync(scriptPath, script);

      const apiToken = requireEnv("AEX_API_TOKEN");
      const passEnv: Record<string, string> = {
        AEX_API_URL: apiUrl,
        AEX_API_TOKEN: apiToken,
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

      const child = await runCommand(process.execPath, [scriptPath], {
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

      expect(result.runtime).toBe("managed");
      expect(result.provider).toBe("deepseek");
      expect(result.runStatus).toBe("succeeded");

      // Real managed-runtime event frame.
      expect(result.eventKinds[0]).toBe("RUN_STARTED");
      expect(result.terminalKind).toBe("RUN_FINISHED");
      expect(result.eventKinds[result.eventKinds.length - 1]).toBe("RUN_FINISHED");
      expect(result.assistantTextEventCount).toBeGreaterThan(0);
      expect(result.assistantTextJoined.length).toBeGreaterThan(0);
      // Strip whitespace before matching the probe — managed-runtime stream
      // fragments responses across content blocks (per token), so the
      // joined text may have spaces in the middle of the probe.
      expect(result.assistantTextJoined.replace(/\s+/g, "")).toContain(result.probe);

      // Terminal carries reason: "complete". runtimeExitCode is NOT asserted
      // here — runner.mjs emits a terminal with { runtimeExitCode } AFTER the
      // managed-runtime process exits, but the runtime adapter's transformObject
      // ("complete") path emits a terminal with { totalTokens } as soon as
      // the managed runtime stdout emits the complete record, and the adapter's
      // idempotency guard drops the second emit. The race winner is the
      // stdout path on a clean run, so runtimeExitCode is normally absent.
      // The previous `if (runtimeExitCode !== undefined) expect(===0)` was a
      // silent-skip anti-pattern. Tightening the contract (defer terminal
      // emission to the runner so BOTH signals land on the same event) is
      // tracked as a managed-runtime refactor — until then, runtimeExitCode is
      // observability, not contract, and not asserted.
      const terminal = result.terminalData ?? {};
      expect(terminal["reason"]).toBe("complete");

      expect(result.leakedProviderKey).toBe(false);
    },
    11 * 60 * 1000
  );
});
