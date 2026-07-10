/**
 * Live scenario: live-sdk-doubao.test.ts
 *
 * Drives the **published `@aexhq/sdk` SDK** against the live
 * api.aex.dev hosted API with a real Doubao (ByteDance) round-trip on the
 * managed runtime: SDK → /sessions → control-plane workflow → managed runtime →
 * real managed-runtime process → BYOK provider-proxy → official Ark API →
 * stream-json events → terminal. No smoke shortcut.
 *
 * This is a per-provider correctness round-trip: it proves the doubao
 * adapter/routing/registry wiring reaches the real Ark upstream and returns a
 * valid response. Feature depth (skills, MCP, AGENTS.md, files) is already
 * covered on the two wire shapes by the DeepSeek (openai-chat) and Anthropic
 * (anthropic-messages) workhorse suites; doubao is openai-chat, so it needs
 * only this connectivity check, not the full scenario matrix.
 *
 * It lives under test/live/providers/ — the on-demand provider suite that is
 * EXCLUDED from the default `test:user` sweep and sessions only via
 * `test:user:providers` (see vitest.providers.config.ts and the manually
 * dispatched .github/workflows/live-on-demand-tests.yml, which sessions every
 * optional suite in one trigger), so the per-provider matrix never piles spend
 * onto every push. It is also the live provider evidence for `doubao`
 * (provider-support.ts). Defaults to the cheap Seed 1.6 Flash tier and the
 * international BytePlus gateway; set AEX_USER_TEST_DOUBAO_PROVIDER=doubao-cn to
 * exercise the China Volcengine gateway instead.
 *
 * Required env:
 *   AEX_API_URL              live api.aex.dev URL
 *   AEX_API_KEY            workspace API key
 *   DOUBAO_API_KEY           customer's Ark (BytePlus/Volcengine) API key
 *   AEX_USER_TEST_TARBALL          path to a packed aex tgz
 *     OR AEX_USER_TEST_VERSION     published package version
 *
 * Optional:
 *   AEX_USER_TEST_DOUBAO_MODEL      default "doubao-seed-flash"
 *   AEX_USER_TEST_DOUBAO_PROVIDER   default "doubao" (or "doubao-cn")
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(
      `user-tests live: required env ${name} is missing. The live Doubao scenario must run against a real api.aex.dev URL with a real Ark key.`
    );
  }
  return value;
}

const doubaoKey = requireEnv("DOUBAO_API_KEY");
const model = process.env["AEX_USER_TEST_DOUBAO_MODEL"] ?? "doubao-seed-flash";
const provider = process.env["AEX_USER_TEST_DOUBAO_PROVIDER"] ?? "doubao";

interface LiveResult {
  readonly sessionId: string;
  readonly sessionStatus: string;
  readonly probe: string;
  readonly eventCount: number;
  readonly eventKinds: readonly string[];
  readonly assistantTextJoined: string;
  readonly assistantTextEventCount: number;
  readonly terminalKind: string | null;
  readonly terminalData: Record<string, unknown> | null;
  readonly fileCount: number;
  readonly files: ReadonlyArray<{ readonly filename: string; readonly sizeBytes: number }>;
  readonly leakedDoubaoKey: boolean;
}

function liveFailureDiagnostic(result: LiveResult): string {
  const safe = {
    ...result,
    assistantTextJoined:
      result.assistantTextJoined.length > 1000
        ? result.assistantTextJoined.slice(0, 1000) + "...[truncated]"
        : result.assistantTextJoined
  };
  const serialized = JSON.stringify(safe, null, 2);
  return serialized.split(doubaoKey).join("[REDACTED_DOUBAO_KEY]");
}

describe("live api.aex.dev via installed SDK — Doubao round-trip on managed runtime", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "submits via SDK, waits for terminal, fetches events + files, asserts a real Doubao response landed in the event log",
    async () => {
      // Drive the SDK from a child Bun process whose cwd is the install
      // tempdir, so `import "@aexhq/sdk"` resolves to the installed tarball —
      // not the monorepo workspace symlink.
      const probe = "e2e-marker-" + Math.random().toString(36).slice(2, 8);
      const script = `
        import { Aex } from "@aexhq/sdk";

        const apiBase = process.env.AEX_API_URL;
        const doubaoKey = process.env.DOUBAO_KEY;
        const model = process.env.MODEL;
        const provider = process.env.PROVIDER;
        const apiKey = process.env.AEX_API_KEY;

        const client = new Aex({
          baseUrl: apiBase,
          apiKey
        });

        const result = await client.start({
          provider,
          model,
          message: ${JSON.stringify(`SessionFile verbatim: ${probe}`)},
          idempotencyKey: "user-test-doubao-" + Date.now(),
          apiKeys: { doubao: doubaoKey }
        }, { timeoutMs: 8 * 60 * 1000 });
        const sessionId = result.sessionId;
        const run = {
          status: result.ok ? "succeeded" : (typeof result.status === "string" && result.status ? result.status : "failed"),
          runtime: "managed",
          provider
        };

        const events = Array.isArray(result.events) ? result.events : [];
        const files = Array.isArray(result.files) ? result.files : [];
        const assistantTextEvents = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT");
        const assistantTextJoined = assistantTextEvents
          .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
          .join(" ");
        const terminal = events.find((e) => (e.type === "TURN_FINISHED" || e.type === "TURN_ERROR"));

        const serialized = JSON.stringify({ run, events, files });
        const payload = {
          sessionId: sessionId,
          sessionStatus: run.status,
          probe: ${JSON.stringify(probe)},
          eventCount: events.length,
          eventKinds: events.map((e) => e.type),
          assistantTextJoined,
          assistantTextEventCount: assistantTextEvents.length,
          terminalKind: terminal ? terminal.type : null,
          terminalData: terminal ? terminal.data : null,
          fileCount: files.length,
          files: files.map((o) => ({ filename: o.filename, sizeBytes: o.sizeBytes })),
          leakedDoubaoKey: serialized.includes(doubaoKey)
        };
        process.stdout.write(JSON.stringify(payload));
        process.exit(0);
      `;
      const scriptPath = join(install.installDir, "live-doubao-runner.mjs");
      writeFileSync(scriptPath, script);

      const apiUrl = requireEnv("AEX_API_URL");
      const apiKey = requireEnv("AEX_API_KEY");
      const passEnv: Record<string, string> = {
        AEX_API_URL: apiUrl,
        AEX_API_KEY: apiKey,
        DOUBAO_KEY: doubaoKey,
        MODEL: model,
        PROVIDER: provider
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

      // ---- assertions ----
      const diagnostic = liveFailureDiagnostic(result);

      expect(result.sessionStatus, diagnostic).toBe("succeeded");
      expect(result.terminalKind, diagnostic).toBe("TURN_FINISHED");
      expect(result.eventKinds, diagnostic).toContain("TURN_STARTED");
      expect(result.eventKinds, diagnostic).toContain("TURN_FINISHED");
      expect(result.eventKinds.indexOf("TURN_STARTED"), diagnostic).toBeLessThan(result.eventKinds.lastIndexOf("TURN_FINISHED"));
      expect(result.assistantTextEventCount, diagnostic).toBeGreaterThan(0);
      expect(result.assistantTextJoined.length, diagnostic).toBeGreaterThan(0);
      // The managed runtime stream fragments responses across content blocks,
      // so strip whitespace before checking probe presence.
      const normalized = result.assistantTextJoined.replace(/\s+/g, "");
      expect(normalized, diagnostic).toContain(result.probe);

      const terminal = result.terminalData ?? {};
      expect(terminal["reason"], diagnostic).toBe("complete");

      // The customer's Ark key MUST NOT appear anywhere in the SDK-visible
      // response surface.
      expect(result.leakedDoubaoKey, diagnostic).toBe(false);
    },
    11 * 60 * 1000
  );
});
