/**
 * Live scenario: live-sdk-anthropic-managed.test.ts
 *
 * Sibling of live-sdk-deepseek.test.ts. Same Goose Managed runtime
 * path, swapped provider — Anthropic via the BYOK provider-proxy
 * (NOT the Anthropic Native runtime, which has its own sibling test).
 *
 *   SDK → POST /api/runs { provider: "anthropic", runtime: "managed" }
 *      → hosted run-lifecycle → managed runtime
 *      → /provider-proxy/anthropic-messages/v1/messages
 *        (Goose dials ANTHROPIC_HOST; gooseProvider="anthropic" → native
 *         shape, NOT the OpenAI-compat layer, which antpath no longer
 *         serves for Anthropic — it loses prompt caching)
 *      → hosted API injects vault'd Anthropic key
 *      → api.anthropic.com /v1/messages
 *      → real claude-haiku-4-5 response → assistant_text event
 *
 * The customer explicitly opts into runtime:'managed' to skip the
 * Anthropic Native default — useful when the customer wants the same
 * Fly-per-run sandbox semantics for Anthropic as they get for other
 * providers (e.g. uniform output capture + cleanup behaviour). The
 * dispatcher rejects feature-runtime mismatches (Skill.provider on
 * managed), so this path is for runs that don't need Anthropic's
 * Skills/Files API.
 *
 * Required env:
 *   ANTPATH_API_URL              live api.antpath.ai URL
 *   ANTHROPIC_API_KEY    customer's Anthropic API key
 *   ANTPATH_USER_TEST_TARBALL          path to packed antpath tgz
 *     OR ANTPATH_USER_TEST_VERSION     published version on npm
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(
      `user-tests live: required env ${name} is missing. The live Anthropic-Managed scenario must run against a real api.antpath.ai URL with a real Anthropic key.`
    );
  }
  return value;
}

const apiUrl = requireEnv("ANTPATH_API_URL");
const anthropicKey = requireEnv("ANTHROPIC_API_KEY");
const model = process.env["ANTPATH_USER_TEST_ANTHROPIC_MODEL"] ?? "claude-haiku-4-5";

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
  // Captured for diagnostic dumps so a missing-deliverable failure is
  // self-describing without a re-run. With no user outputDirs supplied
  // here, this list is expected to be empty — the runner's `.goose-logs`
  // diagnostics now live in the separate `logs` namespace (see below),
  // not in `outputs`.
  readonly outputs: ReadonlyArray<{ readonly filename: string; readonly sizeBytes: number }>;
  // Diagnostic filenames in the `logs` namespace (GET /api/runs/:id/logs,
  // via SDK `getRunDebugLogs`). The runner always emits the three
  // `goose-logs/{stdout.log,stderr.log,args.json}` artifacts here.
  readonly debugLogNames: readonly string[];
  readonly leakedAnthropicKey: boolean;
}

describe("live api.antpath.ai via installed SDK — Anthropic round-trip on Goose Managed runtime (runtime:'managed' opt-out)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAntpath();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "submits via SDK with runtime:'managed', waits for terminal, asserts a real Anthropic response landed in the event log",
    async () => {
      const probe = "e2e-marker-" + Math.random().toString(36).slice(2, 8);
      const script = `
        import { AntpathClient } from "antpath";

        const apiBase = process.env.ANTPATH_API_URL;
        const anthropicKey = process.env.ANTHROPIC_KEY;
        const model = process.env.MODEL;
        const apiToken = process.env.ANTPATH_API_TOKEN;

        const client = new AntpathClient({
          baseUrl: apiBase,
          apiToken
        });

        const runId = await client.submitRun({
          provider: "anthropic",
          runtime: "managed",
          model,
          prompt: ${JSON.stringify(`Output verbatim: ${probe}`)},
          idempotencyKey: "user-test-anthropic-mgd-" + Date.now(),
          secrets: { anthropic: { apiKey: anthropicKey } }
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
        // Diagnostics (.goose-logs/*) now live in the physically-separate
        // logs namespace — fetch them via the same SDK API the
        // download-namespaces reference test uses.
        const debug = await client.getRunDebugLogs(runId);

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
          debugLogNames: debug.logs.map((l) => l.filename),
          leakedAnthropicKey: serialized.includes(anthropicKey)
        };
        process.stdout.write(JSON.stringify(result));
      `;
      const scriptPath = join(install.installDir, "live-anthropic-managed-runner.mjs");
      writeFileSync(scriptPath, script);

      const apiToken = requireEnv("ANTPATH_API_TOKEN");
      const passEnv: Record<string, string> = {
        ANTPATH_API_URL: apiUrl,
        ANTPATH_API_TOKEN: apiToken,
        ANTHROPIC_KEY: anthropicKey,
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
      expect(result.provider).toBe("anthropic");
      expect(result.runStatus).toBe("succeeded");

      // Real-Goose event frame.
      expect(result.eventKinds[0]).toBe("RUN_STARTED");
      expect(result.terminalKind).toBe("RUN_FINISHED");
      expect(result.eventKinds[result.eventKinds.length - 1]).toBe("RUN_FINISHED");
      expect(result.assistantTextEventCount).toBeGreaterThan(0);
      expect(result.assistantTextJoined.length).toBeGreaterThan(0);
      // Strip whitespace before matching the probe — goose stream-json
      // fragments responses across content blocks (per token), so the
      // joined text may have spaces in the middle of the probe.
      expect(result.assistantTextJoined.replace(/\s+/g, "")).toContain(result.probe);

      // Terminal carries reason: "complete". gooseExitCode is NOT asserted
      // here — runner.mjs emits a terminal with { gooseExitCode } AFTER the
      // Goose process exits, but the goose-adapter's transformObject
      // ("complete") path emits a terminal with { totalTokens } as soon as
      // Goose's stdout emits the complete record, and the adapter's
      // idempotency guard drops the second emit. The race winner is the
      // stdout path on a clean run, so gooseExitCode is normally absent.
      // The previous `if (gooseExitCode !== undefined) expect(===0)` was a
      // silent-skip anti-pattern. Tightening the contract (defer terminal
      // emission to the runner so BOTH signals land on the same event) is
      // tracked as a managed-runtime refactor — until then, gooseExitCode is
      // observability, not contract, and not asserted.
      const terminal = result.terminalData ?? {};
      expect(terminal["reason"]).toBe("complete");

      // Run-artifact namespace split: customer deliverables live in the
      // `outputs` namespace; the runner's always-on diagnostics
      // (.goose-logs/{stdout.log,stderr.log,args.json}) live in the
      // physically-separate `logs` namespace. No user outputDirs are
      // supplied here, so `outputs` is empty and the diagnostic-coverage
      // check (>= the 3 the runner always emits) moves to the logs
      // namespace. Dump both listings on failure so a missing artifact is
      // self-describing without a re-run. The runner's outputsSummary is
      // NOT on the SDK-visible terminal event (the goose-adapter's
      // `complete` terminal wins the idempotency race and carries
      // { reason, totalTokens }, not { outputs }), so it isn't dumped here.
      const ctx =
        `outputs=${JSON.stringify(result.outputs)} ` +
        `debugLogNames=${JSON.stringify(result.debugLogNames)}`;
      expect(result.debugLogNames.filter((n) => n.startsWith("goose-logs/")).length, ctx).toBeGreaterThanOrEqual(3);
      expect(result.leakedAnthropicKey).toBe(false);
    },
    11 * 60 * 1000
  );
});
