/**
 * Live scenario: live-sdk-anthropic-managed.test.ts
 *
 * Per-provider correctness round-trip for Anthropic (the anthropic-messages
 * wire shape) — same installed SDK and managed runtime path as the DeepSeek
 * gate suite, swapped provider, via the BYOK provider-proxy. It lives under
 * test/live/providers/ — the on-demand provider suite EXCLUDED from the
 * default release-gating `test:user` sweep (see vitest.providers.config.ts);
 * it runs only via `test:user:providers` (live-on-demand-tests.yml), so the
 * release gate never depends on the Anthropic account billing state.
 *
 *   SDK → POST /api/sessions { provider: "anthropic" }
 *      → hosted session-lifecycle → managed runtime
 *      → /provider-proxy/anthropic-messages/v1/messages
 *      → hosted API injects the session-scoped Anthropic key
 *      → api.anthropic.com /v1/messages → assistant_text event
 *
 * There is no customer runtime selector; every provider uses the same
 * managed sandbox semantics. The dispatcher rejects provider-hosted skill
 * refs, so this path uses local SDK/object storage assets when skills or
 * files are needed.
 *
 * Required env:
 *   AEX_API_URL              live api.aex.dev URL
 *   AEX_API_KEY            workspace API key
 *   ANTHROPIC_API_KEY        customer's Anthropic API key
 *   AEX_USER_TEST_TARBALL          path to packed aex tgz
 *     OR AEX_USER_TEST_VERSION     published package version
 *
 * Optional:
 *   AEX_USER_TEST_ANTHROPIC_MODEL  default "claude-haiku-4-5"
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(
      `user-tests live: required env ${name} is missing. The live Anthropic managed scenario must run against a real api.aex.dev URL with a real Anthropic key.`
    );
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const anthropicKey = requireEnv("ANTHROPIC_API_KEY");
const model = process.env["AEX_USER_TEST_ANTHROPIC_MODEL"]?.trim() || "claude-haiku-4-5";

interface LiveResult {
  readonly sessionId: string;
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
  readonly fileCount: number;
  // Per-file filenames + sizes returned by GET /api/sessions/:id/files.
  // Captured for diagnostic dumps so a deliverable-related failure is
  // self-describing without a retry. Namespace separation is covered by
  // live-sdk-download-namespaces.test.ts; this simple text round-trip does not
  // assert that the model/runtime produced no user deliverables.
  readonly files: ReadonlyArray<{ readonly filename: string; readonly sizeBytes: number }>;
  readonly leakedProviderKey: boolean;
}

function liveFailureDiagnostic(result: LiveResult): string {
  const safe = {
    ...result,
    assistantTextJoined:
      result.assistantTextJoined.length > 1000
        ? result.assistantTextJoined.slice(0, 1000) + "...[truncated]"
        : result.assistantTextJoined
  };
  return JSON.stringify(safe, null, 2).split(anthropicKey).join("[REDACTED_ANTHROPIC_KEY]");
}

describe("live api.aex.dev via installed SDK — Anthropic round-trip on managed runtime", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "submits via SDK, waits for terminal, asserts a real Anthropic response landed in the event log",
    async () => {
      const probe = "e2e-marker-" + Math.random().toString(36).slice(2, 8);
      const script = `
        import { Aex } from "@aexhq/sdk";

        const apiBase = process.env.AEX_API_URL;
        const anthropicKey = process.env.ANTHROPIC_KEY;
        const model = process.env.MODEL;
        const apiKey = process.env.AEX_API_KEY;

        const client = new Aex({
          baseUrl: apiBase,
          apiKey
        });

        const sessionResult = await client.start({
          provider: "anthropic",
          model,
          message: ${JSON.stringify(`Reply with exactly the following token and nothing else, character for character: ${probe}`)},
          idempotencyKey: "user-test-anthropic-mgd-" + Date.now(),
          apiKeys: { anthropic: anthropicKey }
        }, { timeoutMs: 8 * 60 * 1000 });
        const sessionId = sessionResult.sessionId;
        const run = {
          status: sessionResult.status,
          runtime: "managed",
          provider: "anthropic"
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
          runtime: run.runtime ?? "(missing)",
          provider: run.provider ?? "(missing)",
          probe: ${JSON.stringify(probe)},
          eventCount: events.length,
          eventKinds,
          assistantTextJoined,
          assistantTextEventCount: assistantTextEvents.length,
          terminalKind: terminal ? terminal.type : null,
          terminalData,
          fileCount: files.length,
          files: files.map((o) => ({ filename: o.filename, sizeBytes: o.sizeBytes })),
          leakedProviderKey: serialized.includes(anthropicKey)
        };
        process.stdout.write(JSON.stringify(result));
        process.exit(0);
      `;
      const scriptPath = join(install.installDir, "live-anthropic-managed-runner.mjs");
      writeFileSync(scriptPath, script);

      const apiKey = requireEnv("AEX_API_KEY");
      const passEnv: Record<string, string> = {
        AEX_API_URL: apiUrl,
        AEX_API_KEY: apiKey,
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
      const diagnostic = liveFailureDiagnostic(result);

      expect(result.runtime, diagnostic).toBe("managed");
      expect(result.provider, diagnostic).toBe("anthropic");
      expect(result.runStatus, diagnostic).toBe("succeeded");

      // Real managed-runtime event frame.
      expect(result.terminalKind, diagnostic).toBe("RUN_FINISHED");
      expect(result.eventKinds, diagnostic).toContain("RUN_FINISHED");
      expect(result.assistantTextEventCount, diagnostic).toBeGreaterThan(0);
      expect(result.assistantTextJoined.length, diagnostic).toBeGreaterThan(0);
      // Strip whitespace before matching the probe — managed-runtime stream
      // fragments responses across content blocks (per token), so the
      // joined text may have spaces in the middle of the probe.
      expect(result.assistantTextJoined.replace(/\s+/g, ""), diagnostic).toContain(result.probe);

      const terminal = result.terminalData ?? {};
      expect(terminal["outcome"], diagnostic).toBe("succeeded");

      expect(result.leakedProviderKey, diagnostic).toBe(false);
    },
    11 * 60 * 1000
  );
});
