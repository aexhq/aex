/**
 * Live edge-case sweep: SESSION WEBHOOKS surface.
 *
 * Surface under test (public @aexhq/sdk against the dev plane):
 *   - Session `webhook: { url }` registration on submission (SessionCreateOptions.webhook).
 *   - `session.webhooks.list()` / `.redeliver(id)` (the per-session delivery ledger).
 *   - Standard-Webhooks HMAC verification helper `verifyAexWebhook(...)`.
 *   - Webhook-URL SSRF protection (submit-time shape gate + delivery-time IP deny).
 *
 * Design notes (cost/safety):
 *   - URL-validation cases use `sessions.create(...)` WITHOUT a turn, so a rejected
 *     (or even accepted-but-unstarted) webhook costs NO billable LLM turn.
 *   - Only the delivery-observation + SSRF-delivery cases spend billable session turns
 *     (tiny `deepseek-v4-flash` prompts). Total billable session turns in this file: 3.
 *   - Secrets are read from env passed to the child; never printed. Leak checks
 *     emit booleans only.
 *
 * Required env: AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(
      `user-tests live: required env ${name} is missing. The webhook edge sweep must execute against a real dev api URL with a real gate-provider key.`
    );
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const providerKey = requireGateKey("edge-webhooks");
const model = gateModel();

/** Spawn a bun child that executes `body` in the install dir; parse its stdout JSON. */
async function runScript<T>(
  install: InstallResult,
  name: string,
  body: string,
  timeoutMs: number
): Promise<T> {
  const scriptPath = join(install.installDir, name);
  writeFileSync(scriptPath, body);
  const passEnv: Record<string, string> = {
    AEX_API_URL: apiUrl,
    AEX_API_KEY: apiKey,
    PROVIDER: GATE_PROVIDER, PROVIDER_KEY: providerKey,
    MODEL: model
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
    timeoutMs,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `webhook edge runner ${name} exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as T;
  } catch {
    throw new Error(`webhook edge runner ${name} produced non-JSON stdout:\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
  }
}

const CLIENT_PREAMBLE = `
import { Aex } from "@aexhq/sdk";
const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
const PROVIDER = process.env.PROVIDER;
const providerKey = process.env.PROVIDER_KEY;
const model = process.env.MODEL;
`;

interface ValidationCaseResult {
  readonly name: string;
  readonly rejected: boolean;
  readonly code?: string;
  readonly errorName?: string;
  readonly status?: number;
  readonly message?: string;
  readonly sessionId?: string;
}
interface ValidationScriptResult {
  readonly cases: readonly ValidationCaseResult[];
  readonly emptyLedger: { readonly ok: boolean; readonly count?: number; readonly message?: string };
}

describe("live hosted - session webhooks edge cases", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "rejects malformed / non-https / userinfo / non-http-scheme / unknown-field webhook URLs at submit (SSRF shape gate); no-webhook ledger is empty",
    async () => {
      const body = `${CLIENT_PREAMBLE}
        const cases = [
          { name: "not-a-url", webhook: { url: "definitely not a url" } },
          { name: "empty", webhook: { url: "" } },
          { name: "http-not-https", webhook: { url: "http://example.com/aex-hook" } },
          { name: "userinfo", webhook: { url: "https://user:pass@example.com/aex-hook" } },
          { name: "ftp-scheme", webhook: { url: "ftp://example.com/aex-hook" } },
          { name: "file-scheme", webhook: { url: "file:///etc/passwd" } },
          { name: "gopher-scheme", webhook: { url: "gopher://127.0.0.1:6379/_INFO" } },
          { name: "unknown-subfield", webhook: { url: "https://example.com/aex-hook", secret: "leak-me" } },
          { name: "huge-url", webhook: { url: "https://example.com/" + "a".repeat(100000) } }
        ];
        const results = [];
        for (const c of cases) {
          try {
            const s = await client.sessions.create({
              provider: PROVIDER,
              model,
              apiKeys: { [PROVIDER]: providerKey },
              webhook: c.webhook
            });
            results.push({ name: c.name, rejected: false, sessionId: s.id });
          } catch (e) {
            results.push({
              name: c.name,
              rejected: true,
              code: (e && typeof e.code === "string") ? e.code : undefined,
              errorName: (e && typeof e.name === "string") ? e.name : undefined,
              status: (e && typeof e.status === "number") ? e.status : undefined,
              message: String((e && e.message) || e).slice(0, 400)
            });
          }
        }
        // no-webhook session -> empty delivery ledger, not an error
        let emptyLedger;
        try {
          const s = await client.sessions.create({ provider: PROVIDER, model, apiKeys: { [PROVIDER]: providerKey } });
          const ledger = await s.webhooks.list();
          emptyLedger = { ok: true, count: Array.isArray(ledger) ? ledger.length : -1 };
        } catch (e) {
          emptyLedger = { ok: false, message: String((e && e.message) || e).slice(0, 300) };
        }
        process.stdout.write(JSON.stringify({ cases: results, emptyLedger }));
        process.exit(0);
      `;
      const out = await runScript<ValidationScriptResult>(install, "wh-validation.mjs", body, 4 * 60 * 1000);
      const byName = new Map(out.cases.map((c) => [c.name, c] as const));

      // Dump the FULL picture first so a single session reveals every case outcome.
      // eslint-disable-next-line no-console
      console.log("sessions.create webhook-URL validation outcomes:", JSON.stringify(out.cases, null, 2));

      // Security-critical + basic validation cases MUST be rejected at submit.
      const mustReject = ["not-a-url", "empty", "http-not-https", "userinfo", "ftp-scheme", "file-scheme", "gopher-scheme"];
      const leakedPastGate = mustReject.filter((name) => byName.get(name)?.rejected !== true);
      // Clean client-errors only (no 5xx / crash) for the ones that DID reject.
      const badStatus = mustReject
        .map((name) => byName.get(name))
        .filter((c): c is ValidationCaseResult => !!c && c.rejected && c.status !== undefined && (c.status < 400 || c.status >= 500));

      // no-webhook ledger is a clean empty list.
      expect(out.emptyLedger.ok, `no-webhook ledger errored: ${out.emptyLedger.message}`).toBe(true);
      expect(out.emptyLedger.count).toBe(0);

      expect(
        badStatus,
        `webhook URL rejections returned non-4xx status: ${JSON.stringify(badStatus)}`
      ).toEqual([]);
      expect(
        leakedPastGate,
        `these malformed/unsafe webhook URLs were ACCEPTED at sessions.create (not rejected by the submit-time shape gate): ` +
          JSON.stringify(leakedPastGate.map((n) => byName.get(n)))
      ).toEqual([]);
    },
    5 * 60 * 1000
  );

  it(
    "valid https webhook: registration accepted, delivery ledger populates, redeliver(real) works, redeliver(bogus) 404s cleanly, no secret leak",
    async () => {
      const body = `${CLIENT_PREAMBLE}
        const probe = "wh-" + Math.random().toString(36).slice(2, 8);
        const sessionResult = await client.start({
          provider: PROVIDER,
          model,
          message: "Reply with exactly the following token and nothing else, character for character: " + probe,
          apiKeys: { [PROVIDER]: providerKey },
          webhook: { url: "https://example.com/aex-webhook-probe" },
          idempotencyKey: "user-test-wh-valid-" + Date.now()
        }, { timeoutMs: 6 * 60 * 1000 });
        const sessionId = sessionResult.sessionId;
        const session = await client.sessions.open(sessionId);

        // Poll the ledger until a delivery row reaches its own terminal state (delivery is an
        // async post-terminal sweep).
        let deliveries = [];
        let sawRow = false;
        const deadline = Date.now() + 150000;
        while (Date.now() < deadline) {
          deliveries = await session.webhooks.list();
          if (Array.isArray(deliveries) && deliveries.length > 0) {
            sawRow = true;
            const d = deliveries[0];
            if (d && ["delivered", "exhausted", "invalid"].includes(d.status)) break;
          }
          await new Promise((r) => setTimeout(r, 5000));
        }

        let redeliverReal = null;
        if (deliveries.length > 0) {
          try { await session.webhooks.redeliver(deliveries[0].id); redeliverReal = { ok: true }; }
          catch (e) { redeliverReal = { ok: false, status: (e && e.status) || null, message: String((e && e.message) || e).slice(0, 300) }; }
        }
        let redeliverBogus = null;
        try {
          await session.webhooks.redeliver("bogus-delivery-id-does-not-exist-000");
          redeliverBogus = { ok: true };
        } catch (e) {
          redeliverBogus = { ok: false, status: (e && e.status) || null, message: String((e && e.message) || e).slice(0, 300) };
        }

        let events = [];
        try { events = await session.events.list(); } catch (e) { events = []; }
        const serialized = JSON.stringify({ deliveries, events });
        const leakedWhsec = serialized.includes("whsec_");
        const leakedProviderKey = providerKey.length > 0 && serialized.includes(providerKey);

        process.stdout.write(JSON.stringify({
          sessionId,
          runId: sessionResult.run?.runId || null,
          turnSeq: sessionResult.run?.turnSeq || null,
          ok: sessionResult.ok,
          status: sessionResult.status,
          sawRow,
          deliveryCount: deliveries.length,
          firstDelivery: deliveries[0] || null,
          redeliverReal,
          redeliverBogus,
          leakedWhsec,
          leakedProviderKey
        }));
        process.exit(0);
      `;
      const out = await runScript<{
        sessionId: string;
        runId: string | null;
        turnSeq: number | null;
        ok: boolean;
        status: string;
        sawRow: boolean;
        deliveryCount: number;
        firstDelivery: {
          id: string;
          runId: string;
          turnSeq: number;
          eventType: "run.finished" | "run.error";
          status: string;
          attemptCount: number;
          lastStatusCode?: number;
          lastError?: string;
        } | null;
        redeliverReal: { ok: boolean; status?: number | null; message?: string } | null;
        redeliverBogus: { ok: boolean; status?: number | null; message?: string } | null;
        leakedWhsec: boolean;
        leakedProviderKey: boolean;
      }>(install, "wh-valid.mjs", body, 9 * 60 * 1000);

      // eslint-disable-next-line no-console
      console.log("valid-webhook outcome:", JSON.stringify(out));

      expect(out.ok, `session did not complete ok: status=${out.status}`).toBe(true);

      // Secret hygiene: neither the workspace signing secret nor the provider key
      // may appear in the ledger or event log.
      expect(out.leakedWhsec, "workspace signing secret (whsec_) leaked into ledger/events").toBe(false);
      expect(out.leakedProviderKey, "provider API key leaked into ledger/events").toBe(false);

      // redeliver on a bogus id must be a CLEAN error, never a crash / 5xx.
      expect(out.redeliverBogus).not.toBeNull();
      expect(out.redeliverBogus!.ok, `redeliver(bogus) unexpectedly succeeded: ${JSON.stringify(out.redeliverBogus)}`).toBe(false);
      // A clean rejection carries a numeric 4xx status. Coalesce a missing status
      // to 0 so an absent status FAILS (rather than being silently skipped).
      const bogusStatus = out.redeliverBogus!.status ?? 0;
      expect(
        bogusStatus >= 400 && bogusStatus < 500,
        `redeliver(bogus) should be a clean 4xx: ${JSON.stringify(out.redeliverBogus)}`
      ).toBe(true);

      // DEFECT PROBE (F23): a session that completes ok WITH a registered webhook must
      // produce a delivery ledger row (the advertised "notify on finish"). On the
      // dev plane no row EVER appears — verified across idle/suspended/deleted for
      // 10+ min, endpoint healthy (HTTP 200 {deliveries:[]}). This assertion fails
      // today (isolating the defect) and goes green once delivery is wired.
      expect(
        out.sawRow,
        `session ${out.sessionId} completed ok=${out.ok} (status=${out.status}) with webhook registered, ` +
          `but NO webhook delivery row was ever enqueued (ledger stayed empty) — advertised terminal ` +
          `webhook delivery does not fire for the SDK one-shot session flow on the dev plane`
      ).toBe(true);

      // Reached only after the sawRow assertion passes (vitest aborts on the first
      // failure), i.e. only when a delivery row exists. Asserted unconditionally so
      // no expect is gated by an `if`. Validates the row shape and that
      // redeliver(real) either succeeds or fails cleanly (4xx).
      expect(out.firstDelivery, "delivery row present but firstDelivery null").not.toBeNull();
      expect(typeof out.firstDelivery!.id).toBe("string");
      expect(out.firstDelivery!.eventType).toBe("run.finished");
      expect(out.firstDelivery!.runId).toBe(out.runId!);
      expect(out.firstDelivery!.turnSeq).toBe(out.turnSeq!);
      expect(out.redeliverReal, "redeliverReal missing despite a delivery row").not.toBeNull();
      const rr = out.redeliverReal!;
      expect(
        rr.ok === true || (rr.status ?? 0) >= 400,
        `redeliver(real) neither succeeded nor failed cleanly (4xx): ${JSON.stringify(rr)}`
      ).toBe(true);
    },
    10 * 60 * 1000
  );

  it(
    "SSRF: internal/metadata callback URLs are refused (at submit OR at delivery) and NEVER delivered (2xx) to the internal address",
    async () => {
      const body = `${CLIENT_PREAMBLE}
        const targets = [
          { name: "metadata-169.254", url: "https://169.254.169.254/latest/meta-data/iam/security-credentials/" },
          { name: "loopback-127.0.0.1", url: "https://127.0.0.1/admin" },
          { name: "http-not-https", url: "http://example.com/aex-hook" }
        ];
        const out = [];
        for (const t of targets) {
          const rec = { name: t.name, url: t.url };
          try {
            const sessionResult = await client.start({
              provider: PROVIDER,
              model,
              message: "SessionFile verbatim: ssrf-probe",
              apiKeys: { [PROVIDER]: providerKey },
              webhook: { url: t.url },
              idempotencyKey: "user-test-wh-ssrf-" + t.name + "-" + Date.now()
            }, { timeoutMs: 5 * 60 * 1000 });
            rec.submitRejected = false;
            rec.runOk = sessionResult.ok;
            const session = await client.sessions.open(sessionResult.sessionId);
            let deliveries = [];
            const deadline = Date.now() + 130000;
            while (Date.now() < deadline) {
              deliveries = await session.webhooks.list();
              const d = Array.isArray(deliveries) ? deliveries[0] : null;
              if (d && (d.attemptCount > 0 || ["delivered", "exhausted", "invalid"].includes(d.status))) break;
              await new Promise((r) => setTimeout(r, 5000));
            }
            rec.deliveryCount = Array.isArray(deliveries) ? deliveries.length : 0;
            rec.firstDelivery = (Array.isArray(deliveries) && deliveries[0]) ? deliveries[0] : null;
          } catch (e) {
            rec.submitRejected = true;
            rec.code = (e && typeof e.code === "string") ? e.code : undefined;
            rec.errorName = (e && typeof e.name === "string") ? e.name : undefined;
            rec.status = (e && typeof e.status === "number") ? e.status : undefined;
            rec.message = String((e && e.message) || e).slice(0, 400);
          }
          out.push(rec);
        }
        process.stdout.write(JSON.stringify(out));
        process.exit(0);
      `;
      const out = await runScript<
        ReadonlyArray<{
          name: string;
          url: string;
          submitRejected?: boolean;
          runOk?: boolean;
          code?: string;
          errorName?: string;
          status?: number;
          message?: string;
          deliveryCount?: number;
          firstDelivery?: {
            id: string;
            status: string;
            attemptCount: number;
            lastStatusCode?: number;
            lastError?: string;
          } | null;
        }>
      >(install, "wh-ssrf.mjs", body, 9 * 60 * 1000);

      // eslint-disable-next-line no-console
      console.log("SSRF outcomes:", JSON.stringify(out, null, 2));
      // NOTE: on dev, webhook delivery never fires at all (see the valid-webhook
      // case), so "no 2xx to an internal address" holds VACUOUSLY here — this
      // does NOT prove a delivery-time IP-deny guard exists. Since the submit-time
      // shape gate is also absent (internal https URLs accepted), SSRF protection
      // is currently UNVERIFIED and would rest entirely on an unproven guard the
      // moment delivery is enabled.

      expect(out.length).toBe(3);
      for (const rec of out) {
        // If rejected by the API, the rejection must be a clean 4xx. The SDK also
        // rejects malformed webhook URLs client-side before HTTP; that is clean
        // when it carries the typed session-config validation code and a webhook.url
        // message. N/A (true) for accepted records.
        const clientSideValidation =
          rec.submitRejected === true &&
          rec.status === undefined &&
          rec.errorName === "SessionConfigValidationError" &&
          rec.code === "SESSION_CONFIG_INVALID" &&
          typeof rec.message === "string" &&
          rec.message.includes("webhook.url");
        const rejectionIsClean =
          !rec.submitRejected ||
          clientSideValidation ||
          (typeof rec.status === "number" && rec.status >= 400 && rec.status < 500);
        expect(
          rejectionIsClean,
          `SSRF ${rec.name}: submit rejection must be a clean 4xx or typed SDK validation: ${JSON.stringify(rec)}`
        ).toBe(true);

        // Security invariant (holds whether the URL was submit-rejected, delivery-
        // refused, or never delivered): the internal/unsafe URL must NEVER be
        // delivered with a 2xx. delivered2xx is false when no delivery row exists.
        const d = rec.firstDelivery ?? null;
        const delivered2xx =
          !!d &&
          d.status === "delivered" &&
          typeof d.lastStatusCode === "number" &&
          d.lastStatusCode >= 200 &&
          d.lastStatusCode < 300;
        expect(
          delivered2xx,
          `SSRF ${rec.name} DELIVERED (2xx) to internal address — CRITICAL: ${JSON.stringify(d)}`
        ).toBe(false);
        expect(
          d ? d.status : "no-delivery",
          `SSRF ${rec.name} delivery marked "delivered" to an internal address: ${JSON.stringify(d)}`
        ).not.toBe("delivered");
      }
    },
    10 * 60 * 1000
  );

  it(
    "verifyAexWebhook round-trips the Standard-Webhooks HMAC scheme (valid=true, tampered/missing/stale=false)",
    async () => {
      const body = `
        import { verifyAexWebhook } from "@aexhq/sdk";
        const rawKey = new Uint8Array([9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 42, 17, 200, 3, 99, 128]);
        const secretB64 = Buffer.from(rawKey).toString("base64");
        const secret = "whsec_" + secretB64;
        const id = "msg_" + Math.random().toString(36).slice(2, 10);
        const ts = Math.floor(Date.now() / 1000).toString();
        const rawBody = JSON.stringify({ type: "run.finished", subject: "run_test", data: { sessionId: "session_test", runId: "run_test", turnSeq: 1, outcome: "succeeded" } });
        const key = await crypto.subtle.importKey("raw", rawKey, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
        const sigBuf = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(id + "." + ts + "." + rawBody));
        const sigB64 = Buffer.from(new Uint8Array(sigBuf)).toString("base64");
        const headers = { "webhook-id": id, "webhook-timestamp": ts, "webhook-signature": "v1," + sigB64 };

        const good = await verifyAexWebhook({ rawBody, headers, secret });
        // Signature also verifiable with the BARE base64 secret (no whsec_ prefix).
        const goodBareSecret = await verifyAexWebhook({ rawBody, headers, secret: secretB64 });
        // Multiple space-delimited signatures (rotation): match ANY.
        const rotated = await verifyAexWebhook({
          rawBody, headers: { ...headers, "webhook-signature": "v1,AAAAdeadbeef v1," + sigB64 }, secret
        });
        const tamperedBody = await verifyAexWebhook({ rawBody: rawBody + "x", headers, secret });
        const tamperedSig = await verifyAexWebhook({ rawBody, headers: { ...headers, "webhook-signature": "v1,AAAA" }, secret });
        const missing = await verifyAexWebhook({ rawBody, headers: {}, secret });
        const staleTs = await verifyAexWebhook({ rawBody, headers: { ...headers, "webhook-timestamp": "1" }, secret });
        const wrongSecret = await verifyAexWebhook({ rawBody, headers, secret: "whsec_" + Buffer.from(new Uint8Array([1,1,1,1])).toString("base64") });

        process.stdout.write(JSON.stringify({
          isFn: typeof verifyAexWebhook === "function",
          good, goodBareSecret, rotated, tamperedBody, tamperedSig, missing, staleTs, wrongSecret
        }));
        process.exit(0);
      `;
      const out = await runScript<{
        isFn: boolean;
        good: boolean;
        goodBareSecret: boolean;
        rotated: boolean;
        tamperedBody: boolean;
        tamperedSig: boolean;
        missing: boolean;
        staleTs: boolean;
        wrongSecret: boolean;
      }>(install, "wh-verify.mjs", body, 60 * 1000);

      // eslint-disable-next-line no-console
      console.log("verifyAexWebhook outcome:", JSON.stringify(out));

      expect(out.isFn).toBe(true);
      expect(out.good, "valid signature did not verify").toBe(true);
      expect(out.goodBareSecret, "bare-base64 secret did not verify").toBe(true);
      expect(out.rotated, "rotation (multi-signature) did not verify").toBe(true);
      expect(out.tamperedBody, "tampered body verified true (must be false)").toBe(false);
      expect(out.tamperedSig, "tampered signature verified true (must be false)").toBe(false);
      expect(out.missing, "missing headers verified true (must be false)").toBe(false);
      expect(out.staleTs, "stale timestamp verified true (must be false)").toBe(false);
      expect(out.wrongSecret, "wrong secret verified true (must be false)").toBe(false);
    },
    2 * 60 * 1000
  );
});
