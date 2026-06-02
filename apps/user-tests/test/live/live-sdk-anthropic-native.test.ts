/**
 * Live scenario: live-sdk-anthropic-native.test.ts
 *
 * The third sibling — the default Anthropic path. No `runtime` field
 * on submitRun means the dispatcher routes to the Anthropic Native
 * runtime: api.antpath.ai drives the Anthropic call directly
 * from the hosted API (no managed host machine, no Goose, no provider-proxy hop),
 * events land in the KV live tail (then the R2 archive at terminal), the
 * SDK reads them through the same /api/runs/:id/events surface.
 *
 *   SDK → POST /api/runs { provider: "anthropic" }   // runtime defaults to "native"
 *      → Inngest run-lifecycle fn (native poll loop)
 *      → Anthropic Managed Agents API: createEnvironment + createAgent +
 *        createSession + send the turn, then poll the session events
 *        endpoint with the customer's vault'd key
 *      → assistant_text + runtime_terminal events pushed into the
 *        KV live tail
 *   SDK → polls /api/runs/:id → succeeded
 *      → listEvents → sees the real Anthropic response
 *
 * Compared to live-sdk-anthropic-managed (Goose Managed + managed runtime +
 * provider-proxy), this path has no Fly cold-start but DOES provision a
 * Managed Agents environment/agent/session, so it settles in ~30-65s
 * (not the sub-10s of the old single /v1/messages implementation).
 *
 * Required env:
 *   ANTPATH_LIVE_API_BASE              live api.antpath.ai URL
 *   ANTPATH_USER_TEST_ANTHROPIC_KEY    customer's Anthropic API key
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
      `user-tests live: required env ${name} is missing. The live Anthropic-Native scenario must run against a real api.antpath.ai URL with a real Anthropic key.`
    );
  }
  return value;
}

const liveApiBase = requireEnv("ANTPATH_LIVE_API_BASE");
const anthropicKey = requireEnv("ANTPATH_USER_TEST_ANTHROPIC_KEY");
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
  readonly outputCount: number;
  readonly leakedAnthropicKey: boolean;
}

describe("live api.antpath.ai via installed SDK — Anthropic round-trip on Anthropic Native runtime (default)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAntpath();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "submits via SDK (no runtime field), defaults to native, gets a real Anthropic response without a managed host machine",
    async () => {
      const probe = "e2e-marker-" + Math.random().toString(36).slice(2, 8);
      const script = `
        import { AntpathClient } from "antpath";

        const apiBase = process.env.ANTPATH_API_BASE;
        const anthropicKey = process.env.ANTHROPIC_KEY;
        const model = process.env.MODEL;
        const apiToken = process.env.ANTPATH_API_TOKEN;

        const client = new AntpathClient({
          baseUrl: apiBase,
          apiToken
        });

        // No 'runtime' field — the dispatcher defaults provider:"anthropic"
        // to runtime:"native".
        const runId = await client.submitRun({
          provider: "anthropic",
          model,
          prompt: ${JSON.stringify(`Output verbatim: ${probe}`)},
          idempotencyKey: "user-test-anthropic-native-" + Date.now(),
          secrets: { anthropic: { apiKey: anthropicKey } }
        });

        // The native runtime is the full Anthropic Managed Agents API
        // (createEnvironment + createAgent + createSession + event poll):
        // it provisions for ~15-20s before the agent responds and settles
        // in ~30-65s — NOT the old single /v1/messages call. Budget 120s,
        // kept under the runner + outer timeouts below.
        const deadline = Date.now() + 120 * 1000;
        let run = null;
        while (Date.now() < deadline) {
          run = await client.getRun(runId);
          if (run.status === "succeeded" || run.status === "failed" || run.status === "cancelled") {
            break;
          }
          await new Promise((r) => setTimeout(r, 1_500));
        }
        if (!run || (run.status !== "succeeded" && run.status !== "failed" && run.status !== "cancelled")) {
          process.stderr.write(JSON.stringify({ kind: "timeout", run }, null, 2));
          process.exit(2);
        }

        const events = await client.listEvents(runId);
        const outputs = await client.listOutputs(runId);

        // \`listOutputs\` returns every R2 object under the run prefix,
        // including the internal diagnostic namespaces the native runtime
        // always writes (e.g. \`.anthropic-debug/files-list.json\`). Those are
        // debug receipts, not customer outputs — the SDK's \`getRunDebugLogs\`
        // classifies them by the same allowlist. Count only customer outputs.
        const INTERNAL_OUTPUT_PREFIXES = [".goose-logs/", ".fly-logs/", ".anthropic-debug/"];
        const customerOutputs = outputs.filter((o) => {
          const f = o && typeof o.filename === "string" ? o.filename : "";
          return !INTERNAL_OUTPUT_PREFIXES.some((p) => f.startsWith(p));
        });

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
          outputCount: customerOutputs.length,
          terminalKind: terminal ? terminal.type : null,
          leakedAnthropicKey: serialized.includes(anthropicKey)
        };
        process.stdout.write(JSON.stringify(result));
      `;
      const scriptPath = join(install.installDir, "live-anthropic-native-runner.mjs");
      writeFileSync(scriptPath, script);

      const apiToken = requireEnv("ANTPATH_LIVE_API_TOKEN");
      const passEnv: Record<string, string> = {
        ANTPATH_API_BASE: liveApiBase,
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
        timeoutMs: 3 * 60 * 1000,
        env: passEnv
      });
      if (child.exitCode !== 0) {
        throw new Error(
          `live runner exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
        );
      }

      const result = JSON.parse(child.stdout.trim()) as LiveResult;

      // Default routing: no runtime field on the submission, dispatcher
      // picked native because provider is anthropic.
      expect(result.runtime).toBe("native");
      expect(result.provider).toBe("anthropic");
      expect(result.runStatus).toBe("succeeded");

      // Anthropic Native event frame: runtime_started, ≥1 assistant_text,
      // runtime_terminal. No Goose, no managed host machine.
      expect(result.eventCount).toBeGreaterThanOrEqual(3);
      expect(result.eventKinds[0]).toBe("RUN_STARTED");
      expect(result.terminalKind).toBe("RUN_FINISHED");
      expect(result.eventKinds[result.eventKinds.length - 1]).toBe("RUN_FINISHED");
      expect(result.assistantTextEventCount).toBeGreaterThan(0);
      expect(result.assistantTextJoined.length).toBeGreaterThan(0);
      // The Anthropic call saw the user's prompt — proves SDK → /runs →
      // Inngest run-lifecycle → Anthropic preserved prompt content.
      // Strip whitespace because streaming responses fragment across
      // content blocks per-token.
      expect(result.assistantTextJoined.replace(/\s+/g, "")).toContain(result.probe);

      expect(result.outputCount).toBe(0);
      expect(result.leakedAnthropicKey).toBe(false);
    },
    4 * 60 * 1000
  );
});
