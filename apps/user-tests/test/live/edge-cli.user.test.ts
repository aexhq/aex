/**
 * Live edge-case sweep for the installed `aex` CLI binary against the DEV plane.
 *
 * Blackbox: installs the packed/published SDK artifact and spawns the shipped
 * `aex` bin against the real API. Focuses on the CLI's real-world day-one
 * surface that the happy-path `live-cli-installed.test.ts` does not
 * cover:
 *   - a real one-shot `aex start --follow` reaches a clean terminal + prints the
 *     assistant text and session id (with a UNICODE prompt round-trip),
 *   - the read verbs (status/events/files/download) work on that session,
 *   - the auth/error paths (bad token -> 401, missing run -> 404) return a clean
 *     JSON error envelope + non-zero exit, NOT a stack trace or a hang,
 *   - no secret (api key or provider key) is ever echoed to stdout/stderr.
 *
 * Billable sessions: exactly ONE (`aex start --follow`); every other case is a
 * read-only or auth call.
 *
 * Required env (wired by session-live.sh): AEX_API_URL, AEX_API_KEY,
 * DEEPSEEK_API_KEY, AEX_USER_TEST_TARBALL.
 */
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { unzipSync } from "fflate";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getAexBinPath, installAex, runCommand, type InstallResult, type SessionResult } from "../_fixtures/install.js";
import { isPreCreateTransportMessage } from "../_fixtures/pre-create-transport.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const v = process.env[name];
  if (!v || v.length === 0) throw new Error(`edge-cli live: required env ${name} is missing`);
  return v;
}

const apiBase = requireEnv("AEX_API_URL").replace(/\/+$/, "");
const apiKey = requireEnv("AEX_API_KEY");
const providerKey = requireGateKey("edge-cli");
const model = gateModel();

// A completed one-shot turn parks the session cleanly (idle/suspended) or, when
// the deployment projects a terminal session status, `succeeded`. Any of these is a
// clean exit-0 outcome. Mirrors live-cli-installed.test.ts.
const SESSION_PARKED_OK = ["idle", "suspended", "succeeded"];

function redact(text: string): string {
  return text.split(apiKey).join("[REDACTED_TOKEN]").split(providerKey).join("[REDACTED_KEY]");
}

function diag(label: string, r: SessionResult): string {
  return redact(`${label} exited ${r.exitCode}\n--- stdout ---\n${r.stdout}\n--- stderr ---\n${r.stderr}`);
}

function assertNoSecretLeak(label: string, r: SessionResult): void {
  const combined = r.stdout + r.stderr;
  expect(combined.includes(apiKey), `${label}: api key leaked to output`).toBe(false);
  expect(combined.includes(providerKey), `${label}: provider key leaked to output`).toBe(false);
}

function parseJsonLines(stdout: string): Record<string, unknown>[] {
  return stdout
    .trim()
    .split(/\r?\n/)
    .filter((l) => l.length > 0)
    .map((l) => JSON.parse(l) as Record<string, unknown>);
}

function eventText(events: readonly Record<string, unknown>[]): string {
  return events
    .filter((e) => e["type"] === "TEXT_MESSAGE_CONTENT")
    .map((e) => {
      const data = e["data"];
      if (!data || typeof data !== "object" || Array.isArray(data)) return "";
      const t = (data as Record<string, unknown>)["text"];
      return typeof t === "string" ? t : "";
    })
    .join("");
}

function customNames(events: readonly Record<string, unknown>[]): string[] {
  return events
    .filter((e) => e["type"] === "CUSTOM")
    .map((e) => {
      const data = e["data"];
      if (!data || typeof data !== "object" || Array.isArray(data)) return "";
      const n = (data as Record<string, unknown>)["name"];
      return typeof n === "string" ? n : "";
    })
    .filter(Boolean);
}

function hasCleanTerminal(events: readonly Record<string, unknown>[]): boolean {
  const kinds = events.map((e) => e["type"]);
  return kinds.includes("TURN_FINISHED") || customNames(events).some((name) => name.startsWith("aex.session."));
}

function looksTransientProvider(text: string): boolean {
  return /transient-provider|assistant_message_no_public_content|provider returned no public assistant content|provider .*retry later/i.test(text);
}

function parseCliError(stderr: string): Record<string, unknown> | null {
  try {
    const parsed = JSON.parse(stderr.trim()) as unknown;
    return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed as Record<string, unknown> : null;
  } catch {
    return null;
  }
}

describe("live DEV plane via installed aex CLI — edge cases", () => {
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
    return await runCommand(binPath, args, { cwd: install.installDir, timeoutMs });
  }

  async function executeCliRead(label: string, args: readonly string[], timeoutMs = 60_000): Promise<SessionResult> {
    const readErrors = new Set(["status_failed", "events_failed", "files_failed", "download_failed"]);
    const maxAttempts = 3;
    let last: SessionResult | null = null;
    for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
      const result = await executeCli(args, timeoutMs);
      last = result;
      const error = parseCliError(result.stderr);
      const errorCode = error?.["error"];
      const code = typeof errorCode === "string" ? errorCode : "";
      const text = [
        error?.["message"],
        error?.["code"],
        error?.["causeCode"],
        result.stderr
      ].filter((value): value is string => typeof value === "string").join(" ");
      if (result.exitCode === 0 || !readErrors.has(code) || !isPreCreateTransportMessage(text) || attempt >= maxAttempts) {
        return result;
      }
      // eslint-disable-next-line no-console
      console.warn(`[edge-cli] ${label} transient read failure; retrying ${attempt + 1}/${maxAttempts}: ${text.slice(0, 240)}`);
      await new Promise((resolve) => setTimeout(resolve, 1_500 * attempt));
    }
    return last!;
  }

  const common = (): string[] => ["--api-key", apiKey, "--aex-url", apiBase];

  // ---------------------------------------------------------------- auth (non-billable)

  it("whoami with a valid token exits 0 and returns a JSON principal without leaking the token", async () => {
    const r = await executeCli(["whoami", ...common()]);
    expect(r.exitCode, diag("aex whoami", r)).toBe(0);
    const me = JSON.parse(r.stdout.trim()) as Record<string, unknown>;
    expect(me, diag("aex whoami", r)).toBeTypeOf("object");
    // whoami must resolve a workspace/principal identity from the bearer alone.
    expect(Object.keys(me).length, diag("aex whoami", r)).toBeGreaterThan(0);
    assertNoSecretLeak("whoami", r);
  });

  it("whoami with a garbage token exits 1 with a clean 4xx JSON envelope (no stack trace)", async () => {
    // A structurally-garbage token is rejected as 400 malformed_token by dev
    // (not 401). The robust contract: non-zero exit + a clean JSON envelope that
    // surfaces the server's reason, never a stack trace or a hang.
    const badToken = "aex_not_a_real_token_deadbeef";
    const r = await executeCli(["whoami", "--api-key", badToken, "--aex-url", apiBase]);
    expect(r.exitCode, diag("aex whoami (garbage token)", r)).toBe(1);
    const err = JSON.parse(r.stderr.trim()) as Record<string, unknown>;
    expect(err["error"], diag("aex whoami (garbage token)", r)).toBe("whoami_failed");
    const status = Number(err["status"] ?? 0);
    expect(status, diag("aex whoami (garbage token)", r)).toBeGreaterThanOrEqual(400);
    expect(status, diag("aex whoami (garbage token)", r)).toBeLessThan(500);
    // the server reason is surfaced so the user is not left blind
    expect(typeof err["message"], diag("aex whoami (garbage token)", r)).toBe("string");
    expect(String(err["message"]).length, diag("aex whoami (garbage token)", r)).toBeGreaterThan(0);
    // The CLI should attach an actionable auth remedy for both malformed and
    // invalid token paths so the user is not left with a bare server reason.
    const remedyText = typeof err["remedy"] === "string" ? (err["remedy"] as string) : "";
    expect(/--api-key|aex login/.test(remedyText), diag("aex whoami (garbage token)", r)).toBe(true);
    // the bad token itself must not be echoed back
    expect(r.stdout + r.stderr).not.toContain(badToken);
    // no raw stack trace
    expect(r.stderr).not.toMatch(/\bat .+\(.+:\d+:\d+\)/);
  });

  it("whoami with a well-formed-but-invalid token surfaces a clean auth error", async () => {
    // Mutate the tail of the REAL token so it keeps the recognized shape but is
    // not a valid credential — this exercises the "recognized format, wrong
    // value" path (typically 401 with an actionable remedy) rather than the
    // 400 malformed path above. We never print the real or mutated token.
    const mutatedTail = apiKey.slice(-6).split("").reverse().join("") === apiKey.slice(-6)
      ? "zzzzzz"
      : apiKey.slice(-6).split("").reverse().join("");
    const mutated = apiKey.slice(0, -6) + mutatedTail;
    // Guard: ensure we actually changed the token.
    expect(mutated).not.toBe(apiKey);
    const r = await executeCli(["whoami", "--api-key", mutated, "--aex-url", apiBase]);
    expect(r.exitCode, diag("aex whoami (mutated token)", r)).toBe(1);
    const err = JSON.parse(r.stderr.trim()) as Record<string, unknown>;
    expect(err["error"], diag("aex whoami (mutated token)", r)).toBe("whoami_failed");
    const status = Number(err["status"] ?? 0);
    expect(status, diag("aex whoami (mutated token)", r)).toBeGreaterThanOrEqual(400);
    expect(status, diag("aex whoami (mutated token)", r)).toBeLessThan(500);
    // The CLI should attach an actionable auth remedy regardless of whether the
    // API classifies the mutated token as malformed (400) or unauthorized (401).
    const remedyText = typeof err["remedy"] === "string" ? (err["remedy"] as string) : "";
    expect(/--api-key|aex login/.test(remedyText), diag("aex whoami (mutated token)", r)).toBe(true);
    // neither the real nor the mutated token may appear in output
    expect(r.stdout + r.stderr).not.toContain(mutated);
    assertNoSecretLeak("whoami-mutated", r);
    expect(r.stderr).not.toMatch(/\bat .+\(.+:\d+:\d+\)/);
  });

  it("status on a nonexistent session id exits 1 with a clean not-found JSON envelope", async () => {
    const r = await executeCli(["status", "session-does-not-exist-000000", ...common()]);
    expect(r.exitCode, diag("aex status (missing id)", r)).toBe(1);
    const err = JSON.parse(r.stderr.trim()) as Record<string, unknown>;
    expect(err["error"], diag("aex status (missing id)", r)).toBe("status_failed");
    // The server may answer 404 (unknown) or 4xx; assert it is a clean client
    // error surfaced as JSON, never a crash. 404 gets the "verify the id" remedy.
    const status = Number(err["status"] ?? 0);
    expect(status, diag("aex status (missing id)", r)).toBeGreaterThanOrEqual(400);
    expect(status, diag("aex status (missing id)", r)).toBeLessThan(500);
    assertNoSecretLeak("status-missing", r);
    expect(r.stderr).not.toMatch(/\bat .+\(.+:\d+:\d+\)/);
  });

  // ---------------------------------------------------------------- the one billable session turn + reads

  it(
    "run --follow with a UNICODE prompt reaches a clean terminal, prints the id + assistant text, and the read verbs work",
    async () => {
      const diagnostics: string[] = [];
      const maxAttempts = 3;

      for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
        const asciiId = `EDGE-${Date.now().toString(36)}-${attempt}-${Math.random().toString(36).slice(2, 8)}`;
        // Unicode round-trip via a UTF-8 prompt file (@path) so the marker is not
        // mangled by the host shell before it reaches the CLI. Tests the CLI's
        // file-read + JSON transmission path for multi-byte content.
        const unicodeMarker = "日本語 café";
        const promptPath = join(install.installDir, `edge-cli-prompt-${asciiId}.txt`);
        writeFileSync(promptPath, `SessionFile verbatim, exactly, with no extra words: ${asciiId} ${unicodeMarker}`, "utf8");

        const run = await executeCli(
          [
            "start",
            "--provider", GATE_PROVIDER,
            "--model", model,
            "--prompt", `@${promptPath}`,
            "--deepseek-api-key", providerKey,
            "--idempotency-key", `edge-cli-${asciiId.toLowerCase()}`,
            "--follow",
            "--timeout", "8m",
            ...common()
          ],
          10 * 60_000
        );
        const runDiag = diag("aex start --follow", run);
        if (run.exitCode !== 0) {
          diagnostics.push(`attempt ${attempt}: ${runDiag}`);
          if (attempt < maxAttempts && looksTransientProvider(runDiag)) continue;
        }
        expect(run.exitCode, `${runDiag}\n\nprior attempts:\n${diagnostics.join("\n\n")}`).toBe(0);
        assertNoSecretLeak("run", run);

        const runLines = parseJsonLines(run.stdout);
        const initial = runLines[0]!;
        const sessionId = initial["id"];
        expect(typeof sessionId, runDiag).toBe("string");
        const finalFromFollow = [...runLines]
          .reverse()
          .find((l) => l["id"] === sessionId && typeof l["status"] === "string");
        expect(SESSION_PARKED_OK, runDiag).toContain(finalFromFollow?.["status"]);

        const id = sessionId as string;

        // status: id + clean status
        const status = await executeCliRead("aex status", ["status", id, ...common()]);
        expect(status.exitCode, diag("aex status", status)).toBe(0);
        const statusDoc = JSON.parse(status.stdout.trim()) as Record<string, unknown>;
        expect(statusDoc["id"], diag("aex status", status)).toBe(id);
        expect(SESSION_PARKED_OK, diag("aex status", status)).toContain(statusDoc["status"]);
        assertNoSecretLeak("status", status);

        // events: TURN_STARTED + clean terminal + the assistant echoed the markers
        const events = await executeCliRead("aex events", ["events", id, ...common()]);
        expect(events.exitCode, diag("aex events", events)).toBe(0);
        const eventRows = parseJsonLines(events.stdout);
        expect(eventRows.map((e) => e["type"]), diag("aex events", events)).toContain("TURN_STARTED");
        expect(hasCleanTerminal(eventRows), diag("aex events", events)).toBe(true);
        const joined = eventText(eventRows).replace(/\s+/g, "");
        expect(joined, diag("aex events", events)).toContain(asciiId);
        // Unicode round-trip: the multi-byte marker survived arg-file -> CLI ->
        // API -> model -> event stream -> CLI stdout decode.
        expect(joined, "unicode marker did not round-trip through the CLI").toContain("日本語");
        expect(joined, "latin-1 marker did not round-trip through the CLI").toContain("café");
        assertNoSecretLeak("events", events);

        // files: exit 0 (list may be empty for a pure text turn)
        const files = await executeCliRead("aex files", ["files", id, ...common()]);
        expect(files.exitCode, diag("aex files", files)).toBe(0);
        const outputRows = files.stdout.trim().length > 0 ? parseJsonLines(files.stdout) : [];
        for (const o of outputRows) expect(typeof o["id"], diag("aex files", files)).toBe("string");
        assertNoSecretLeak("files", files);

        // download --only events -> a real zip with events.jsonl
        const zipPath = join(install.installDir, `edge-cli-events-${id}.zip`);
        const download = await executeCliRead("aex download --only events", ["download", id, "--only", "events", "--out", zipPath, ...common()]);
        expect(download.exitCode, diag("aex download --only events", download)).toBe(0);
        expect(JSON.parse(download.stdout.trim())).toMatchObject({ sessionId: id, namespace: "events", path: zipPath });
        expect(existsSync(zipPath), diag("aex download --only events", download)).toBe(true);
        const entries = unzipSync(new Uint8Array(readFileSync(zipPath)));
        expect(Object.keys(entries)).toContain("events.jsonl");
        assertNoSecretLeak("download", download);
        return;
      }

      throw new Error(`edge CLI live test failed after ${maxAttempts} attempts:\n${diagnostics.join("\n\n")}`);
    },
    30 * 60_000
  );
});
