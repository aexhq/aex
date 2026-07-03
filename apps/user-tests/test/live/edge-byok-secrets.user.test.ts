/**
 * EDGE-CASE SWEEP — BYOK / SECRETS / MULTI-PROVIDER surface (SECURITY-SENSITIVE).
 *
 * Real-customer stress test of the public `@aexhq/sdk` against the DEV plane,
 * focused on the run-submission BYOK surface:
 *   - `apiKeys` map (per-provider BYOK keys)
 *   - `environment.secrets` + the `Secret` primitive (ephemeral `Secret.value`
 *     and workspace `Secret.ref` / `Secret.value(...).upload`)
 *   - `client.secrets` vault (set/list/get/rotate/delete)
 *
 * The overriding invariant under test is NON-LEAKAGE: a provider key or a
 * `secretEnv` value must NEVER appear in the persisted events, outputs, run
 * record, or any customer-readable surface. The platform proves injection of a
 * secret via a SHA-256 digest computed IN the subprocess (the raw value is
 * redacted out of model-facing output by design), so these tests mirror that
 * digest-proof pattern rather than echoing raw secrets.
 *
 * Provider: the DeepSeek gate provider (`deepseek-v4-flash`), tiny prompts. Each `it` spawns a
 * small Bun script in the installed-SDK tempdir; the script imports the SDK,
 * submits, collects, and prints result JSON the test asserts on. Secrets and
 * per-test canaries are passed via the child ENV — never inlined into a script
 * source and never printed.
 *
 * Required env (exported by the live runner): AEX_API_URL, AEX_API_TOKEN,
 * DEEPSEEK_API_KEY, AEX_USER_TEST_TARBALL.
 */
import { createHash, randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(
      `edge-byok-secrets: required env ${name} is missing. Run via the live runner so AEX_API_URL / AEX_API_TOKEN / DEEPSEEK_API_KEY are exported.`
    );
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const providerKey = requireGateKey("edge-byok-secrets");
const model = gateModel();

const RUN_TIMEOUT_MS = 6 * 60_000;
const IT_TIMEOUT_MS = 8 * 60_000;

/** Whitespace-stripped text — streamed assistant events fragment tokens. */
function dense(s: string): string {
  return s.replace(/\s+/g, "");
}

/** sha256 hex of exact bytes (no trailing newline) — matches `printf '%s' | sha256sum`. */
function sha256Hex(value: string): string {
  return createHash("sha256").update(value, "utf8").digest("hex");
}

function rand(prefix: string): string {
  return `${prefix}-${randomBytes(6).toString("hex")}`;
}

/**
 * Run a Bun script in the install dir with the SDK + secrets on the child env.
 * `extraEnv` carries per-test canaries/keys (generated test-side, never printed).
 * The script must print a single JSON object to stdout.
 */
async function runScript(
  install: InstallResult,
  scriptName: string,
  body: string,
  extraEnv: Record<string, string> = {}
): Promise<Record<string, unknown>> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, body);
  const passEnv: Record<string, string> = {
    AEX_API_URL: apiUrl,
    AEX_API_TOKEN: apiToken,
    PROVIDER: GATE_PROVIDER, PROVIDER_KEY: providerKey,
    MODEL: model,
    ...extraEnv
  };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) passEnv[pathKey] = process.env[pathKey]!;
  const carry =
    process.platform === "win32"
      ? ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]
      : ["HOME", "TMPDIR", "LANG", "LC_ALL"];
  for (const k of carry) if (process.env[k]) passEnv[k] = process.env[k]!;

  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs: 7 * 60_000,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-byok script ${scriptName} exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
}

/**
 * Shared in-script preamble: build the client + a `gather(runId)` that pulls
 * EVERY customer-readable surface (events, outputs, session record, messages,
 * text) and a `leakScan(surfaces, needle)` that reports, per surface, whether
 * the raw needle appears. Emitted verbatim into each script.
 */
const PREAMBLE = `
import { Aex, Secret } from "@aexhq/sdk";
const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiToken: process.env.AEX_API_TOKEN });
const PROVIDER = process.env.PROVIDER;
const PROVIDER_KEY = process.env.PROVIDER_KEY;
const MODEL = process.env.MODEL;
async function gather(runId, runResult) {
  let events = Array.isArray(runResult?.events) ? runResult.events : [];
  let outputs = Array.isArray(runResult?.outputs) ? runResult.outputs : [];
  let record = null, messages = [];
  try {
    const s = await client.sessions.open(runId);
    const le = await s.events().list();
    if (Array.isArray(le) && le.length > 0) events = le;
    const lo = await s.outputs().list();
    if (Array.isArray(lo)) outputs = lo;
    try { messages = await s.messages.all(); } catch {}
  } catch {}
  try { record = await client.sessions.get(runId); } catch {}
  return { events, outputs, record, messages };
}
function leakScan(surfaces, needle, extra) {
  const per = {
    events: JSON.stringify(surfaces.events).includes(needle),
    outputs: JSON.stringify(surfaces.outputs).includes(needle),
    record: JSON.stringify(surfaces.record).includes(needle),
    messages: JSON.stringify(surfaces.messages).includes(needle),
    text: String((extra && extra.text) || "").includes(needle)
  };
  const anywhere = per.events || per.outputs || per.record || per.messages || per.text;
  return { leaked: anywhere, per };
}
function assistantText(surfaces, runResult) {
  const fromEvents = surfaces.events
    .filter((e) => e && e.type === "TEXT_MESSAGE_CONTENT")
    .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : ""))
    .join(" ");
  return fromEvents || (typeof runResult?.text === "string" ? runResult.text : "");
}
function statusOf(runResult) {
  return runResult && runResult.ok ? "succeeded" : (runResult && typeof runResult.status === "string" && runResult.status ? runResult.status : "failed");
}
`;

describe("edge/BYOK+secrets — leakage & error-path hardening on the dev plane", () => {
  let install: InstallResult;
  beforeAll(async () => {
    install = await installAex();
  }, 240_000);
  afterAll(() => install?.cleanup());

  // ── CASE A (CRIT): provider key never leaks across ANY surface ────────────
  it(
    "A: apiKeys{gate provider} run — raw provider key appears in NO event/output/record/message",
    async () => {
      const probe = rand("keyleak-probe");
      const body = `${PREAMBLE}
        const probe = process.env.PROBE;
        const runResult = await client.run({
          provider: PROVIDER, model: MODEL,
          message: "Output verbatim: " + probe,
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-keyleak-" + Date.now()
        }, { timeoutMs: ${RUN_TIMEOUT_MS} });
        const runId = runResult.runId;
        const surfaces = await gather(runId, runResult);
        const text = assistantText(surfaces, runResult);
        const scan = leakScan(surfaces, PROVIDER_KEY, { text });
        process.stdout.write(JSON.stringify({
          runId, status: statusOf(runResult), probe, text,
          eventCount: surfaces.events.length, outputCount: surfaces.outputs.length,
          leaked: scan.leaked, per: scan.per
        }));
      `;
      const r = await runScript(install, "edge-a-keyleak.mjs", body, { PROBE: probe });
      expect(r.status).toBe("succeeded");
      expect(dense(String(r.text))).toContain(probe);
      // The CRITICAL assertion: the raw provider key is nowhere customer-readable.
      expect(r.leaked).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  // ── CASE B: ephemeral secretEnv value IS injected (digest proof) & NOT leaked
  it(
    "B: Secret.value in environment.secrets reaches the subprocess (sha256 proof) but the raw value never leaks",
    async () => {
      const canary = rand("ephem-canary");
      const digest = sha256Hex(canary);
      const digestNl = sha256Hex(canary + "\n");
      const body = `${PREAMBLE}
        const CANARY = process.env.CANARY;
        const PROMPT = [
          "Using the shell, run exactly (no trailing newline on the value):",
          "  printf '%s' \\"$CANARY_VALUE\\" | sha256sum",
          "It prints a 64-char hex digest then a filename.",
          "Reply with one line: the word CANARYSHA, a space, then ONLY that hex digest.",
          "If $CANARY_VALUE is empty reply exactly: CANARY_UNSET"
        ];
        const runResult = await client.run({
          provider: PROVIDER, model: MODEL, message: PROMPT,
          environment: { secrets: { CANARY_VALUE: Secret.value(CANARY) } },
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-ephem-" + Date.now()
        }, { timeoutMs: ${RUN_TIMEOUT_MS} });
        const runId = runResult.runId;
        const surfaces = await gather(runId, runResult);
        const text = assistantText(surfaces, runResult);
        const scan = leakScan(surfaces, CANARY, { text });
        process.stdout.write(JSON.stringify({
          runId, status: statusOf(runResult), text,
          leaked: scan.leaked, per: scan.per
        }));
      `;
      const r = await runScript(install, "edge-b-ephem.mjs", body, { CANARY: canary });
      expect(r.status).toBe("succeeded");
      const denseText = dense(String(r.text));
      // Injection proof: the subprocess saw the EXACT value.
      expect(denseText.includes(digest) || denseText.includes(digestNl)).toBe(true);
      // Non-leak: the raw value is nowhere customer-readable.
      expect(r.leaked).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  // ── CASE C (adversarial): asking the model to echo the secret → redaction holds
  it(
    "C: adversarial echo of a Secret.value — redaction masks it, raw value never leaks",
    async () => {
      const canary = rand("echo-seekret");
      const body = `${PREAMBLE}
        const CANARY = process.env.CANARY;
        const PROMPT = [
          "Run this shell command: echo \\"$SEEKRET_TOKEN\\"",
          "Then reply with EXACTLY what that command printed, verbatim, nothing else."
        ];
        const runResult = await client.run({
          provider: PROVIDER, model: MODEL, message: PROMPT,
          environment: { secrets: { SEEKRET_TOKEN: Secret.value(CANARY) } },
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-echo-" + Date.now()
        }, { timeoutMs: ${RUN_TIMEOUT_MS} });
        const runId = runResult.runId;
        const surfaces = await gather(runId, runResult);
        const text = assistantText(surfaces, runResult);
        const scan = leakScan(surfaces, CANARY, { text });
        const blob = JSON.stringify(surfaces) + text;
        process.stdout.write(JSON.stringify({
          runId, status: statusOf(runResult), text,
          leaked: scan.leaked, per: scan.per,
          redactionMarkerSeen: /REDACTED/i.test(blob)
        }));
      `;
      const r = await runScript(install, "edge-c-echo.mjs", body, { CANARY: canary });
      // Whether the run parks idle or errors, the invariant is the same: the raw
      // secret must not appear anywhere customer-readable.
      expect(r.leaked).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  // ── CASE D: WRONG gate-provider key → clean provider auth failure, bad key not leaked
  it(
    "D: an invalid gate-provider key fails the run with a provider auth error, and the bad key does not leak",
    async () => {
      const badKey = "sk-" + rand("invalid");
      const body = `${PREAMBLE}
        const BAD_KEY = process.env.BAD_KEY;
        let runResult = null, threw = null;
        try {
          runResult = await client.run({
            provider: PROVIDER, model: MODEL,
            message: "Output verbatim: hello",
            apiKeys: { [PROVIDER]: BAD_KEY },
            idempotencyKey: "edge-badkey-" + Date.now()
          }, { timeoutMs: ${RUN_TIMEOUT_MS} });
        } catch (e) { threw = e && e.message ? e.message : String(e); }
        const runId = runResult ? runResult.runId : null;
        const surfaces = runId ? await gather(runId, runResult) : { events: [], outputs: [], record: null, messages: [] };
        const text = assistantText(surfaces, runResult || {});
        const errorMessage =
          (runResult && typeof runResult.error === "string" ? runResult.error : "") ||
          (surfaces.record && typeof surfaces.record.errorMessage === "string" ? surfaces.record.errorMessage : "") ||
          threw || "";
        // Scan the WHOLE surface set plus every error string for the bad key.
        const scan = leakScan(surfaces, BAD_KEY, { text: text + " " + errorMessage + " " + (threw || "") });
        // Look for a stream_error / error event as an additional failure signal.
        const errorEventKinds = surfaces.events
          .filter((e) => e && (e.type === "RUN_ERROR" || (e.type === "CUSTOM" && e.data && (e.data.name === "aex.stream_error" || e.data.name === "aex.session.error"))))
          .map((e) => e.type + (e.data && e.data.name ? ":" + e.data.name : ""));
        process.stdout.write(JSON.stringify({
          runId, status: statusOf(runResult), threw,
          errorMessage, errorEventKinds,
          eventKinds: surfaces.events.map((e) => e && e.type),
          leaked: scan.leaked, per: scan.per
        }));
      `;
      const r = await runScript(install, "edge-d-badkey.mjs", body, { BAD_KEY: badKey });
      // Must NOT succeed.
      expect(r.status).not.toBe("succeeded");
      // Bad key must not leak into any surface, error text, or thrown message.
      expect(r.leaked).toBe(false);
      // There must be SOME error signal (not a silent hang / empty success).
      const errSignal =
        String(r.errorMessage || "") + String(r.threw || "") +
        (Array.isArray(r.errorEventKinds) ? (r.errorEventKinds as string[]).join(",") : "");
      expect(errSignal.length).toBeGreaterThan(0);
    },
    IT_TIMEOUT_MS
  );

  // ── CASE E (client-side): MISSING apiKeys for the gate provider → fast clear error, no hang
  it(
    "E: submitting the gate provider with no apiKeys throws a clear client-side error before any run is billed",
    async () => {
      const body = `${PREAMBLE}
        const t0 = Date.now();
        let threw = null, runId = null;
        try {
          const rr = await client.run({
            provider: PROVIDER, model: MODEL, message: "hi",
            idempotencyKey: "edge-missingkey-" + Date.now()
          }, { timeoutMs: 30000 });
          runId = rr.runId;
        } catch (e) { threw = e && e.message ? e.message : String(e); }
        process.stdout.write(JSON.stringify({ threw, runId, elapsedMs: Date.now() - t0 }));
      `;
      const r = await runScript(install, "edge-e-missingkey.mjs", body);
      expect(r.runId).toBeNull();
      expect(String(r.threw || "")).toMatch(/api key is required|apiKeys/i);
      // Fast-fail: a missing key must not hang on the network.
      expect(Number(r.elapsedMs)).toBeLessThan(30_000);
    },
    60_000
  );

  // ── CASE F: multi-provider map with an extra UNUSED key → accepted, unused key not leaked
  it(
    "F: an unused extra provider key in apiKeys is accepted and never leaks",
    async () => {
      const probe = rand("multiprov-probe");
      const unusedKey = "sk-ant-unused-" + rand("x");
      const body = `${PREAMBLE}
        const probe = process.env.PROBE;
        const UNUSED_KEY = process.env.UNUSED_KEY;
        const runResult = await client.run({
          provider: PROVIDER, model: MODEL,
          message: "Output verbatim: " + probe,
          apiKeys: { [PROVIDER]: PROVIDER_KEY, anthropic: UNUSED_KEY },
          idempotencyKey: "edge-multiprov-" + Date.now()
        }, { timeoutMs: ${RUN_TIMEOUT_MS} });
        const runId = runResult.runId;
        const surfaces = await gather(runId, runResult);
        const text = assistantText(surfaces, runResult);
        const scanUnused = leakScan(surfaces, UNUSED_KEY, { text });
        const scanReal = leakScan(surfaces, PROVIDER_KEY, { text });
        process.stdout.write(JSON.stringify({
          runId, status: statusOf(runResult), text,
          unusedLeaked: scanUnused.leaked, realLeaked: scanReal.leaked
        }));
      `;
      const r = await runScript(install, "edge-f-multiprov.mjs", body, { PROBE: probe, UNUSED_KEY: unusedKey });
      expect(r.status).toBe("succeeded");
      expect(dense(String(r.text))).toContain(probe);
      expect(r.unusedLeaked).toBe(false);
      expect(r.realLeaked).toBe(false);
    },
    IT_TIMEOUT_MS
  );

  // ── CASE G: workspace secret round-trip (Secret.value.upload → Secret.ref in secretEnv)
  it(
    "G: a workspace secret (upload → ref) referenced via secretEnv — injection behaviour + non-leak on dev",
    async () => {
      const name = rand("wscanary").replace(/-/g, "_"); // SECRET_HANDLE_PATTERN safe
      const canary = rand("wsval");
      const digest = sha256Hex(canary);
      const digestNl = sha256Hex(canary + "\n");
      const body = `${PREAMBLE}
        const NAME = process.env.WS_NAME;
        const CANARY = process.env.CANARY;
        let uploadErr = null, ref = null;
        try { ref = await Secret.value(CANARY).upload(client, { name: NAME }); }
        catch (e) { uploadErr = e && e.message ? e.message : String(e); }
        let secretRecord = null;
        try { secretRecord = await client.secrets.get(NAME); } catch (e) { secretRecord = { getError: e && e.message ? e.message : String(e) }; }
        const PROMPT = [
          "Using the shell, run exactly (no trailing newline on the value):",
          "  printf '%s' \\"$WS_CANARY\\" | sha256sum",
          "Reply with one line: the word CANARYSHA, a space, then ONLY that hex digest.",
          "If $WS_CANARY is empty reply exactly: CANARY_UNSET"
        ];
        let runResult = null, runErr = null;
        try {
          runResult = await client.run({
            provider: PROVIDER, model: MODEL, message: PROMPT,
            environment: { secrets: { WS_CANARY: ref || Secret.ref(NAME) } },
            apiKeys: { [PROVIDER]: PROVIDER_KEY },
            idempotencyKey: "edge-wsref-" + Date.now()
          }, { timeoutMs: ${RUN_TIMEOUT_MS} });
        } catch (e) { runErr = e && e.message ? e.message : String(e); }
        const runId = runResult ? runResult.runId : null;
        const surfaces = runId ? await gather(runId, runResult) : { events: [], outputs: [], record: null, messages: [] };
        const text = assistantText(surfaces, runResult || {});
        const scan = leakScan(surfaces, CANARY, { text });
        // metadata read must never contain the value either
        const metaLeak = JSON.stringify(secretRecord).includes(CANARY);
        try { await client.secrets.delete(NAME); } catch {}
        process.stdout.write(JSON.stringify({
          runId, status: statusOf(runResult), text, uploadErr, runErr,
          secretRecord, metaLeak, leaked: scan.leaked, per: scan.per
        }));
      `;
      const r = await runScript(install, "edge-g-wsref.mjs", body, { WS_NAME: name, CANARY: canary });
      // The upload + workspace-secret create/get/delete cycle must work and the
      // stored value must never come back in metadata.
      expect(r.uploadErr).toBeNull();
      expect(r.metaLeak).toBe(false);
      // Non-leak invariant holds regardless of whether the ref resolves.
      expect(r.leaked).toBe(false);
      // Injection result is recorded for the report (dev Phase-1 does NOT seal
      // workspace-ref decls — only ephemeral values — so this is expected to be
      // false on dev; a match would mean ref-resolution is wired).
      const denseText = dense(String(r.text));
      const injected = denseText.includes(digest) || denseText.includes(digestNl);
      const unset = /CANARY_UNSET/.test(String(r.text));
      // Assert the run itself is healthy (submitted + reached terminal) so a
      // false `injected` is a resolution gap, not an infra failure.
      expect(r.status === "succeeded" || r.status === "error" || r.status === "failed").toBe(true);
      // Document: on dev we expect NO injection (ref not sealed) → agent sees unset.
      expect(injected || unset).toBe(true);
    },
    IT_TIMEOUT_MS
  );

  // ── CASE H: non-existent secret ref handle in secretEnv → observe error vs silent-unset
  it(
    "H: referencing a non-existent workspace secret handle — no crash; behaviour observed",
    async () => {
      const ghost = rand("ghost").replace(/-/g, "_");
      const probe = rand("ghost-probe");
      const body = `${PREAMBLE}
        const GHOST = process.env.GHOST;
        const probe = process.env.PROBE;
        let runResult = null, submitOrRunErr = null;
        try {
          runResult = await client.run({
            provider: PROVIDER, model: MODEL,
            message: "Output verbatim: " + probe,
            environment: { secrets: { GHOST_VAR: Secret.ref(GHOST) } },
            apiKeys: { [PROVIDER]: PROVIDER_KEY },
            idempotencyKey: "edge-ghost-" + Date.now()
          }, { timeoutMs: ${RUN_TIMEOUT_MS} });
        } catch (e) { submitOrRunErr = e && e.message ? e.message : String(e); }
        const runId = runResult ? runResult.runId : null;
        const surfaces = runId ? await gather(runId, runResult) : { events: [], outputs: [], record: null, messages: [] };
        const text = assistantText(surfaces, runResult || {});
        process.stdout.write(JSON.stringify({
          runId, status: statusOf(runResult), submitOrRunErr,
          text, probePresent: text.replace(/\\s+/g,"").includes(probe)
        }));
      `;
      const r = await runScript(install, "edge-h-ghost.mjs", body, { GHOST: ghost, PROBE: probe });
      // Whatever the platform chooses (reject at submit, fail the run, or proceed
      // with an unset var), it must NOT crash the SDK harness. Exactly one of the
      // two contract-valid outcomes must hold:
      //   (a) a clear error was surfaced, or
      //   (b) the run proceeded to a terminal status.
      const surfacedError = typeof r.submitOrRunErr === "string" && r.submitOrRunErr.length > 0;
      const reachedTerminal = r.status === "succeeded" || r.status === "error" || r.status === "failed";
      expect(surfacedError || reachedTerminal).toBe(true);
    },
    IT_TIMEOUT_MS
  );

  // ── CASE I (client-side): Secret primitive input validation guards
  it(
    "I: Secret.ref / Secret.value reject malformed input at the call site (no network)",
    async () => {
      const body = `${PREAMBLE}
        const out = {};
        try { Secret.ref("bad handle with spaces!"); out.badHandle = "NO_THROW"; }
        catch (e) { out.badHandle = e && e.message ? e.message : String(e); }
        try { Secret.value(""); out.emptyValue = "NO_THROW"; }
        catch (e) { out.emptyValue = e && e.message ? e.message : String(e); }
        try { const s = Secret.value("real-value"); out.toStringRedacted = s.toString(); out.toJson = JSON.stringify(s); }
        catch (e) { out.valueErr = e && e.message ? e.message : String(e); }
        try { const s = Secret.ref("good_handle.name-1"); out.goodRef = s.handle; }
        catch (e) { out.goodRef = "THREW:" + (e && e.message ? e.message : String(e)); }
        process.stdout.write(JSON.stringify(out));
      `;
      const r = await runScript(install, "edge-i-validation.mjs", body);
      expect(String(r.badHandle)).not.toBe("NO_THROW");
      expect(String(r.badHandle)).toMatch(/handle must match/i);
      expect(String(r.emptyValue)).not.toBe("NO_THROW");
      expect(String(r.emptyValue)).toMatch(/non-empty|required/i);
      expect(r.goodRef).toBe("good_handle.name-1");
      // A Secret must NEVER serialize its raw value.
      expect(String(r.toStringRedacted)).not.toContain("real-value");
      expect(String(r.toJson)).not.toContain("real-value");
    },
    60_000
  );
});
