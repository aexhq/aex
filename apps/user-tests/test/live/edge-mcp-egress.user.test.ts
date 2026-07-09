/**
 * EDGE-CASE USER TEST (SDK-driven, customer perspective) —
 * `McpServer` primitive, MCP declaration validation, SSRF handling, and
 * container EGRESS ALLOWLIST enforcement (security-critical for launch).
 *
 * Surface under test:
 *   - SDK `McpServer.remote(...)` / `McpServer.fromId(...)` construction guards
 *     (offline: missing/empty fields, stdio rejection, header/secret split,
 *     workspace-id pattern).
 *   - A session declaring a private/metadata/duplicate/bad-name MCP ref must FAIL
 *     CLOSED without ever dialing the target and without leaking any cloud
 *     metadata. (On the dev plane this is enforced as a session error at the
 *     first turn — see the "observed behavior" note below.)
 *   - The container egress firewall on a `networking:limited` session: the declared
 *     host stays reachable while a NON-allowlisted host AND cloud metadata
 *     (169.254.169.254) are BLOCKED. A reachable non-allowlisted host or a
 *     reachable IMDS would be a CRITICAL defect.
 *   - Secret MCP headers do not leak into the session event/output log.
 *
 * OBSERVED BEHAVIOR (dev, 2026-07-02): invalid MCP refs are ACCEPTED at
 * submission (HTTP 200 — a session is created) and only rejected ~45s later as a
 * `aex.session.failed` carrying the canonical `parseMcpServerRef` deny reason.
 * Security holds (fail-closed, target never dialed, no metadata leak), but the
 * documented "SSRF guard at the parser boundary" is NOT applied synchronously
 * at the API create endpoint. These fail-closed sessions invoke no LLM (they error
 * during setup); only the egress + MCP-invocation cases spend model tokens.
 *
 * Required env (wired by the live runner):
 *   AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY,
 *   AEX_USER_TEST_TARBALL | AEX_USER_TEST_VERSION
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-mcp-egress): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const providerKey = requireGateKey("edge-mcp-egress");
const model = gateModel();

// DeepWiki — public, unauthenticated remote MCP (same upstream the existing
// mcp-invocation test uses). MCP hosts are always allowlisted through egress.
const MCP_NAME = "deepwiki";
const MCP_URL = "https://mcp.deepwiki.com/mcp";

function buildPassEnv(extras: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extras };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  const carry =
    process.platform === "win32"
      ? ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]
      : ["HOME", "TMPDIR", "LANG", "LC_ALL"];
  for (const k of carry) if (process.env[k]) env[k] = process.env[k]!;
  return env;
}

async function runChild(install: InstallResult, scriptName: string, script: string, timeoutMs: number): Promise<string> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, script);
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: buildPassEnv({ AEX_API_URL: apiUrl, AEX_API_KEY: apiKey, PROVIDER: GATE_PROVIDER, PROVIDER_KEY: providerKey, MODEL: model })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `${scriptName} runner exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  return child.stdout.trim();
}

// ---------------------------------------------------------------------------
// Construction guards (pure offline) + bad-submission fail-closed probes
// (submitted in PARALLEL so all resolve within one ~45s window). The child
// ALWAYS exits 0 and prints one JSON blob the `it`s assert on.
// ---------------------------------------------------------------------------
interface ConstructionCase {
  readonly label: string;
  readonly threw: boolean;
  readonly message: string;
  readonly extra?: unknown;
}
interface SubmissionCase {
  readonly label: string;
  readonly resolveMs: number;
  readonly sessionId: string | null;
  readonly status: string | null;
  readonly failureClass: string | null;
  readonly errorMessage: string | null;
  readonly threw: string | null;
  readonly reason: string | null;
  readonly dialed: boolean;
  readonly metaLeak: readonly string[];
}
interface ValidationResult {
  readonly construction: readonly ConstructionCase[];
  readonly submission: readonly SubmissionCase[];
}

function validationChildScript(): string {
  return `
    import { Aex, McpServer, MCP_SERVER_NAME_PATTERN } from "@aexhq/sdk";

    const construction = [];
    function attempt(label, fn) {
      try {
        const value = fn();
        construction.push({ label, threw: false, message: "", extra: value ?? null });
      } catch (err) {
        construction.push({ label, threw: true, message: err && err.message ? String(err.message) : String(err) });
      }
    }

    // --- pure construction guards (no network) ---
    attempt("valid-remote-with-headers", () => {
      const m = McpServer.remote({ name: "deepwiki", url: "https://mcp.deepwiki.com/mcp", headers: { Authorization: "Bearer XYZ" } });
      return { sub: m.toSubmissionEntry(), sec: m.toSecretEntry() ?? null };
    });
    attempt("missing-url", () => McpServer.remote({ name: "noturl" }));
    attempt("missing-name", () => McpServer.remote({ url: "https://example.com/mcp" }));
    attempt("empty-name", () => McpServer.remote({ name: "", url: "https://example.com/mcp" }));
    attempt("stdio-transport", () => new McpServer({ kind: "inline", name: "x", url: "https://x.com/mcp", transport: "stdio" }));
    attempt("stdio-command-field", () => new McpServer({ name: "x", url: "https://x.com/mcp", command: "node" }));
    attempt("bad-name-uppercase", () => {
      const m = McpServer.remote({ name: "UPPER_CASE", url: "https://example.com/mcp" });
      return { patternMatches: MCP_SERVER_NAME_PATTERN.test("UPPER_CASE"), constructedName: m.name };
    });
    attempt("fromId-bad", () => McpServer.fromId("not-a-valid-id"));
    attempt("fromId-good", () => {
      const m = McpServer.fromId("mcp_abcdefgh12345678");
      return { kind: m.kind, id: m.id };
    });

    // --- bad-submission fail-closed probes (parallel; no LLM is invoked) ---
    const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
    async function submitBad(label, mcpServers) {
      const t0 = Date.now();
      let sessionId = null, status = null, threw = null;
      let failureClass = null, errorMessage = null;
      try {
        const res = await client.start({
          provider: process.env.PROVIDER,
          model: process.env.MODEL,
          message: "Output verbatim: EDGE",
          mcpServers,
          includeBuiltinTools: false,
          apiKeys: { [process.env.PROVIDER]: process.env.PROVIDER_KEY },
          idempotencyKey: "edge-mcp-" + label + "-" + Date.now()
        }, { timeoutMs: 120000 });
        sessionId = res && typeof res.sessionId === "string" ? res.sessionId : null;
        status = res && typeof res.status === "string" ? res.status : (res && res.ok ? "succeeded" : null);
      } catch (err) {
        threw = (err && err.name ? err.name : "Error") + "/status=" + (err && err.status) + ": " + (err && err.message ? String(err.message).slice(0, 200) : String(err));
      }
      let reason = null, dialed = false, metaLeak = [];
      if (sessionId) {
        try {
          const s = await client.sessions.open(sessionId);
          try {
            const rec = await client.sessions.get(sessionId);
            status = typeof rec.status === "string" ? rec.status : status;
            failureClass = typeof rec.failureClass === "string" ? rec.failureClass : null;
            errorMessage = typeof rec.errorMessage === "string" ? rec.errorMessage : null;
            if (reason === null && errorMessage !== null) reason = errorMessage;
          } catch {}
          const events = await s.events().list();
          const errEvt = events.find((e) => e.type === "CUSTOM" && e.data && ["aex.session.failed", "aex.session.timed_out", "aex.session.cancelled"].includes(String(e.data.name || "")))
            || events.find((e) => e.type === "CUSTOM" && e.data && String(e.data.name || "").includes("error"))
            || events.find((e) => e.type === "TURN_ERROR");
          if (errEvt && reason === null) reason = (errEvt.data && errEvt.data.value && errEvt.data.value.reason) || JSON.stringify(errEvt.data).slice(0, 300);
          dialed = events.some((e) => e.type === "TOOL_CALL_START");
          const serialized = JSON.stringify(events);
          // Real IMDS content markers (the deny-reason text legitimately
          // contains "169.254", so we look for actual metadata payload).
          for (const m of ["ami-id", "instance-id", "security-credentials", "iam/", "\\"hostname\\":\\"ip-"]) {
            if (serialized.includes(m)) metaLeak.push(m);
          }
        } catch (e) { reason = "inspect-err:" + String(e && e.message).slice(0, 120); }
      }
      return { label, resolveMs: Date.now() - t0, sessionId, status, failureClass, errorMessage, threw, reason, dialed, metaLeak };
    }

    const submission = await Promise.all([
      submitBad("ssrf-imds",          [McpServer.remote({ name: "imds", url: "http://169.254.169.254/latest/meta-data/" })]),
      submitBad("ssrf-rfc1918",       [McpServer.remote({ name: "priv", url: "http://10.0.0.1/mcp" })]),
      submitBad("bad-name-uppercase", [McpServer.remote({ name: "UPPER_CASE", url: "https://mcp.deepwiki.com/mcp" })]),
      submitBad("duplicate-names",    [McpServer.remote({ name: "dup", url: "https://mcp.deepwiki.com/mcp" }), McpServer.remote({ name: "dup", url: "https://mcp.deepwiki.com/mcp" })])
    ]);

    process.stdout.write(JSON.stringify({ construction, submission }));
    process.exit(0);
  `;
}

// ---------------------------------------------------------------------------
// Standard event-collection tail reused by the billable-session children.
// ---------------------------------------------------------------------------
const COLLECT = `
  const sessionId = sessionResult.sessionId;
  const status = sessionResult.ok ? "succeeded" : (typeof sessionResult.status === "string" && sessionResult.status ? sessionResult.status : "failed");
  const fallbackEvents = Array.isArray(sessionResult.events) ? sessionResult.events : [];
  let events = fallbackEvents;
  let outputs = Array.isArray(sessionResult.outputs) ? sessionResult.outputs : [];
  try {
    const session = await client.sessions.open(sessionId);
    const listedEvents = await session.events().list();
    if (Array.isArray(listedEvents) && listedEvents.length > 0) events = listedEvents;
    const listedOutputs = await session.outputs().list();
    if (Array.isArray(listedOutputs)) outputs = listedOutputs;
  } catch {}
  const eventKinds = events.map((e) => e.type);
  const assistantText = events.filter((e) => e.type === "TEXT_MESSAGE_CONTENT")
    .map((e) => (e.data && typeof e.data.text === "string" ? e.data.text : "")).join(" ");
  const toolResultText = events.filter((e) => e.type === "TOOL_CALL_RESULT")
    .map((e) => JSON.stringify(e && e.data !== undefined ? e.data : "")).join(" ");
  const toolRequests = events.filter((e) => e.type === "TOOL_CALL_START").map((e) => ({
    name: e.data && typeof e.data.name === "string" ? e.data.name : null,
    extension: e.data && typeof e.data.extension === "string" ? e.data.extension : null
  }));
  const toolResponseCount = events.filter((e) => e.type === "TOOL_CALL_RESULT").length;
  const streamErrors = events.filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error")
    .map((e) => JSON.stringify(e.data).slice(0, 400));
  const serialized = JSON.stringify({ events, outputs });
`;

interface EgressResult {
  readonly sessionId: string;
  readonly status: string;
  readonly eventKinds: readonly string[];
  readonly evidence: string;
  readonly streamErrors: readonly string[];
}

function egressChildScript(): string {
  const probe = `set -u
probe_https() {
  label="$1"
  url="$2"
  body="/tmp/aex-egress-$label.body"
  err="/tmp/aex-egress-$label.err"
  code="$(curl -sS -m 8 -o "$body" -w "%{http_code}" "$url" 2>"$err" || true)"
  if [ -n "$code" ] && [ "$code" != "000" ]; then
    printf "%s_REACHED_HTTP_%s\\n" "$label" "$code"
  else
    printf "%s_BLOCKED_HTTP_%s\\n" "$label" "\${code:-000}"
  fi
}
probe_imds() {
  body="/tmp/aex-egress-imds.body"
  headers="/tmp/aex-egress-imds.headers"
  err="/tmp/aex-egress-imds.err"
  code="$(curl -sS -m 8 -D "$headers" -o "$body" -w "%{http_code}" http://169.254.169.254/latest/meta-data/ 2>"$err" || true)"
  text="$(cat "$headers" "$body" "$err" 2>/dev/null | tr "A-Z" "a-z" || true)"
  if [ -z "$code" ] || [ "$code" = "000" ]; then
    printf "IMDS_BLOCKED_HTTP_%s\\n" "\${code:-000}"
  elif [ "$code" = "403" ] && printf "%s" "$text" | grep -Eq "proxy|forbidden|plain-http|egress|not permitted|metadata|link-local"; then
    printf "IMDS_BLOCKED_HTTP_%s\\n" "$code"
  else
    printf "IMDS_REACHED_HTTP_%s\\n" "$code"
  fi
}
a="$(probe_https ALLOWED https://example.com/)"
o="$(probe_https OTHER https://api.github.com/)"
i="$(probe_imds)"
printf "%s %s %s\\n" "$a" "$o" "$i"`;
  const prompt =
    "Using the shell, execute exactly this bash script without replacing it with curl exit-code shortcuts. " +
    "Classify reachability from HTTP status and proxy/gate response text: proxy 403/Forbidden, connection refused, HTTP 000, or timeout means BLOCKED. " +
    "A real IMDS response, including IMDSv2 401, means IMDS_REACHED. " +
    `Script:\n${probe}\n` +
    "Reply with ONLY the final three probe tokens separated by single spaces.";
  return `
    import { Aex } from "@aexhq/sdk";
    const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
    const sessionResult = await client.start({
      provider: process.env.PROVIDER,
      model: process.env.MODEL,
      message: ${JSON.stringify(prompt)},
      environment: { networking: { mode: "limited", allowedHosts: ["example.com"] } },
      apiKeys: { [process.env.PROVIDER]: process.env.PROVIDER_KEY },
      idempotencyKey: "edge-egress-" + Date.now()
    }, { timeoutMs: 8 * 60000 });
    ${COLLECT}
    const evidence = (toolResultText + " " + assistantText).replace(/\\s+/g, "");
    process.stdout.write(JSON.stringify({ sessionId, status, eventKinds, evidence, streamErrors }));
    process.exit(0);
  `;
}

interface McpSecretResult {
  readonly sessionId: string;
  readonly status: string;
  readonly eventKinds: readonly string[];
  readonly toolRequests: ReadonlyArray<{ name: string | null; extension: string | null }>;
  readonly toolResponseCount: number;
  readonly assistantText: string;
  readonly secretLeaked: boolean;
  readonly streamErrors: readonly string[];
}

function mcpSecretChildScript(marker: string): string {
  const prompt =
    `Use the ${MCP_NAME} tool to look up the primary programming language of the GitHub ` +
    `repository anthropics/anthropic-cookbook. Reply with exactly one line: lang=<language>.`;
  return `
    import { Aex, McpServer } from "@aexhq/sdk";
    const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
    // Secret header carried under secrets.mcpServers — must never surface in events/outputs.
    const mcp = McpServer.remote({
      name: ${JSON.stringify(MCP_NAME)},
      url: ${JSON.stringify(MCP_URL)},
      headers: { "X-Aex-Secret-Probe": ${JSON.stringify(marker)} }
    });
    // The non-secret submission entry must NOT contain the marker.
    const subEntry = JSON.stringify(mcp.toSubmissionEntry());
    const sessionResult = await client.start({
      provider: process.env.PROVIDER,
      model: process.env.MODEL,
      message: ${JSON.stringify(prompt)},
      mcpServers: [mcp],
      includeBuiltinTools: false,
      apiKeys: { [process.env.PROVIDER]: process.env.PROVIDER_KEY },
      idempotencyKey: "edge-mcp-secret-" + Date.now()
    }, { timeoutMs: 8 * 60000 });
    ${COLLECT}
    const marker = ${JSON.stringify(marker)};
    const secretLeaked = serialized.includes(marker) || subEntry.includes(marker);
    process.stdout.write(JSON.stringify({ sessionId, status, eventKinds, toolRequests, toolResponseCount, assistantText, secretLeaked, streamErrors }));
    process.exit(0);
  `;
}

// ---------------------------------------------------------------------------

let install: InstallResult;
let validation: ValidationResult;

function ctor(label: string): ConstructionCase {
  const c = validation.construction.find((x) => x.label === label);
  if (!c) throw new Error(`construction case ${label} missing; got: ${validation.construction.map((x) => x.label).join(", ")}`);
  return c;
}
function sub(label: string): SubmissionCase {
  const s = validation.submission.find((x) => x.label === label);
  if (!s) throw new Error(`submission case ${label} missing; got: ${validation.submission.map((x) => x.label).join(", ")}`);
  return s;
}
/** A bad MCP ref must FAIL CLOSED: erroring/rejected, never dialing, no metadata leak. */
function assertFailedClosed(s: SubmissionCase): void {
  const dump = JSON.stringify(s);
  // The target must NEVER be dialed and no cloud-metadata payload may appear.
  expect(s.dialed, `bad MCP ref DIALED its target — SSRF breach: ${dump}`).toBe(false);
  expect(s.metaLeak, `cloud-metadata content leaked: ${dump}`).toEqual([]);
  // The session must not have succeeded on a bad ref.
  expect(s.status === "succeeded", `bad MCP ref session SUCCEEDED (should fail closed): ${dump}`).toBe(false);
  // It must be rejected somewhere: either a thrown API error OR an error terminal.
  const rejected = s.threw !== null || s.status === "error" || s.status === "failed" || s.reason !== null;
  expect(rejected, `bad MCP ref was neither rejected nor errored: ${dump}`).toBe(true);
}

function rejectionText(s: SubmissionCase): string {
  return `${s.reason ?? ""} ${s.errorMessage ?? ""} ${s.failureClass ?? ""} ${s.threw ?? ""}`.toLowerCase();
}

describe("edge: McpServer primitive + MCP declaration + egress allowlist (security)", () => {
  beforeAll(async () => {
    install = await installAex();
    const out = await runChild(install, "edge-validation.mjs", validationChildScript(), 180_000);
    validation = JSON.parse(out) as ValidationResult;
  }, 300_000);
  afterAll(() => install?.cleanup());

  // ---- Construction guards (offline) ----
  it("valid McpServer.remote(+headers) constructs; submission entry omits the header", () => {
    const withHeaders = ctor("valid-remote-with-headers");
    expect(withHeaders.threw, withHeaders.message).toBe(false);
    const e = withHeaders.extra as { sub: Record<string, unknown>; sec: Record<string, unknown> | null };
    // Non-secret wire entry must be {name,url} only — never carry the header.
    expect(Object.keys(e.sub).sort()).toEqual(["name", "url"]);
    expect(JSON.stringify(e.sub)).not.toContain("Authorization");
    // Secret entry carries the header out-of-band.
    expect(e.sec && JSON.stringify(e.sec)).toContain("Authorization");
  });

  it("missing url / missing name / empty name throw clear construction errors", () => {
    expect(ctor("missing-url").threw).toBe(true);
    expect(ctor("missing-url").message).toMatch(/url is required/i);
    expect(ctor("missing-name").threw).toBe(true);
    expect(ctor("missing-name").message).toMatch(/name is required/i);
    expect(ctor("empty-name").threw).toBe(true);
  });

  it("stdio-shaped MCP inputs are rejected at construction (transport + command field)", () => {
    expect(ctor("stdio-transport").threw).toBe(true);
    expect(ctor("stdio-command-field").threw).toBe(true);
  });

  it("McpServer.fromId validates the workspace id pattern", () => {
    expect(ctor("fromId-bad").threw).toBe(true);
    expect(ctor("fromId-good").threw, ctor("fromId-good").message).toBe(false);
  });

  it("DX note: SDK constructor does NOT pre-validate the MCP name pattern (server enforces it)", () => {
    // Documents the client-side gap: an invalid name constructs fine; the
    // canonical pattern is only enforced server-side (asserted below).
    const c = ctor("bad-name-uppercase");
    expect(c.threw).toBe(false);
    const e = c.extra as { patternMatches: boolean; constructedName: string };
    expect(e.patternMatches).toBe(false);
    expect(e.constructedName).toBe("UPPER_CASE");
  });

  // ---- Bad-submission fail-closed (security) ----
  it("SECURITY: an MCP url at cloud metadata 169.254.169.254 fails closed and is never dialed", () => {
    const s = sub("ssrf-imds");
    assertFailedClosed(s);
    // The deny reason must reference the metadata / link-local range.
    expect(rejectionText(s), JSON.stringify(s)).toMatch(/169\.254|metadata|link-local/);
  });

  it("SECURITY: an MCP url at a private RFC1918 address fails closed and is never dialed", () => {
    const s = sub("ssrf-rfc1918");
    assertFailedClosed(s);
    expect(rejectionText(s), JSON.stringify(s)).toMatch(/rfc1918|10\.0\.0\.0|private/);
  });

  it("an MCP name violating MCP_SERVER_NAME_PATTERN is rejected server-side (fail closed)", () => {
    const s = sub("bad-name-uppercase");
    assertFailedClosed(s);
    expect(rejectionText(s), JSON.stringify(s)).toMatch(/must match|name/i);
  });

  it("duplicate MCP server names are rejected server-side (fail closed)", () => {
    const s = sub("duplicate-names");
    assertFailedClosed(s);
    expect(rejectionText(s), JSON.stringify(s)).toMatch(/duplicate/);
  });

  // ---- Egress allowlist enforcement (KEY security check, 1 LLM turn) ----
  it(
    "SECURITY: networking:limited blocks non-allowlisted host AND cloud metadata, allows the declared host",
    async () => {
      const out = await runChild(install, "edge-egress.mjs", egressChildScript(), 9 * 60_000);
      const r = JSON.parse(out) as EgressResult;
      const dump = JSON.stringify({ sessionId: r.sessionId, status: r.status, eventKinds: r.eventKinds, streamErrors: r.streamErrors, evidence: r.evidence.slice(0, 800) });
      // SessionRecord itself succeeds — platform proxy/model host is always allowed.
      expect(r.status, dump).toBe("succeeded");
      // Allowed host reachable.
      expect(r.evidence, dump).toContain("ALLOWED_REACHED");
      // Non-allowlisted host BLOCKED (a reachable one is a firewall breach).
      expect(r.evidence, `non-allowlisted host reachable — egress firewall breach: ${dump}`).toContain("OTHER_BLOCKED");
      expect(r.evidence, `non-allowlisted host reachable — egress firewall breach: ${dump}`).not.toContain("OTHER_REACHED");
      // IMDS BLOCKED (a reachable one is a CRITICAL SSRF/credential-exfil defect).
      expect(r.evidence, `CLOUD METADATA REACHABLE — critical: ${dump}`).toContain("IMDS_BLOCKED");
      expect(r.evidence, `CLOUD METADATA REACHABLE — critical: ${dump}`).not.toContain("IMDS_REACHED");
      // Ground-truth guard: no IMDS metadata content in the real tool output.
      expect(r.evidence.toLowerCase(), `IMDS metadata content leaked into tool output: ${dump}`)
        .not.toMatch(/ami-id|instance-id|security-credentials|iam\//);
    },
    10 * 60_000
  );

  // ---- MCP invocation + secret-header non-leak (1 LLM turn) ----
  it(
    "SECURITY: a remote MCP is invoked and its secret header never leaks into events/outputs",
    async () => {
      const marker = "AEXSECRET" + Math.random().toString(36).slice(2, 12).toUpperCase();
      const out = await runChild(install, "edge-mcp-secret.mjs", mcpSecretChildScript(marker), 9 * 60_000);
      const r = JSON.parse(out) as McpSecretResult;
      const dump = JSON.stringify({
        sessionId: r.sessionId, status: r.status, eventKinds: r.eventKinds,
        toolRequests: r.toolRequests, toolResponseCount: r.toolResponseCount,
        streamErrors: r.streamErrors, assistantText: r.assistantText.slice(0, 300)
      });
      // The security invariant — a secret MCP header must NEVER appear in the
      // event/output log or the non-secret submission entry.
      expect(r.secretLeaked, `SECRET MCP HEADER LEAKED into session event/output log: ${dump}`).toBe(false);
      expect(r.status, dump).toBe("succeeded");
      // The MCP was actually reached (with the secret header applied out-of-band).
      const mcpMatch = (v: string | null): boolean =>
        !!v && v.toLowerCase().replace(/[^a-z0-9]+/g, "").includes(MCP_NAME);
      const invoked = r.toolRequests.some((t) => mcpMatch(t.name) || mcpMatch(t.extension));
      expect(invoked, `no MCP tool_request observed (secret header may have broken the connection): ${dump}`).toBe(true);
      expect(r.toolResponseCount, `MCP tool_request had no correlated response: ${dump}`).toBeGreaterThan(0);
    },
    10 * 60_000
  );
});
