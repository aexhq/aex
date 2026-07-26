import fc from "fast-check";
import { afterAll, describe, expect, it } from "bun:test";

/**
 * LIVE adversarial-input fuzz of the deployed dev API (the public HTTP contract
 * a customer's @aexhq/sdk hits). RAW fetch — no SDK, no mocks — so we can send
 * malformed bytes the SDK would never emit. Substrate-agnostic: the same
 * robustness invariants hold for any deployment plane behind the public contract.
 *
 * Fails fast unless AEX_API_URL + AEX_API_KEY are set. Run on demand via
 *   bun run --filter @aexhq/user-tests test:user:fuzz
 * (excluded from the default `test:user` sweep — see scripts/user-bun-test.mjs).
 *
 * COST SAFETY: the only POST /api/sessions bodies sent are ones that are GUARANTEED
 * to be rejected BEFORE any run is dispatched (invalid JSON, non-object JSON,
 * stdio MCP, SSRF/non-https webhook). No well-formed submission is ever sent, so
 * the fuzz never spawns a real (Fargate-billed) run.
 *
 * INVARIANTS asserted:
 *   (a) bad input ⇒ a structured 4xx, NEVER a 5xx;
 *   (b) auth enforced: no/garbage bearer ⇒ 401; valid bearer ⇒ whoami 200;
 *   (c) region-token routing: any crafted aex_* token ⇒ {308,401,403,451}, never 5xx;
 *   (d) reject determinism: the same malformed submit ⇒ the same status (no dup);
 *   (e) reads with adversarial ids/queries ⇒ 4xx or 2xx, never 5xx.
 */

interface FuzzEnv {
  readonly base: string;
  readonly token: string;
  readonly sessions: number;
}

function requireFuzzEnv(): FuzzEnv {
  const rawBase = process.env.AEX_API_URL;
  if (!rawBase) {
    throw new Error("live-api-fuzz: required env AEX_API_URL is missing");
  }
  const base = rawBase.replace(/\/+$/, "");
  if (!/^https?:\/\//.test(base)) {
    throw new Error("live-api-fuzz: AEX_API_URL must be an absolute http(s) URL");
  }
  const token = process.env.AEX_API_KEY;
  if (!token) {
    throw new Error("live-api-fuzz: required env AEX_API_KEY is missing");
  }
  const sessions = Number(process.env.AEX_FUZZ_RUNS ?? "60");
  if (!Number.isInteger(sessions) || sessions < 1) {
    throw new Error("live-api-fuzz: AEX_FUZZ_RUNS must be a positive integer when set");
  }
  return { base, token, sessions };
}

const { base: BASE, token: TOKEN, sessions: SESSIONS } = requireFuzzEnv();

// --- single-attempt raw transport -------------------------------------------

interface Res {
  status: number;
  body: string;
}
const findings: string[] = [];

async function call(
  method: string,
  path: string,
  opts: { token?: string | null; body?: string; contentType?: string; redirect?: RequestRedirect } = {}
): Promise<Res> {
  const headers: Record<string, string> = { accept: "application/json" };
  if (opts.token) headers.authorization = `Bearer ${opts.token}`;
  if (opts.body !== undefined) headers["content-type"] = opts.contentType ?? "application/json";
  try {
    const response = await fetch(`${BASE}${path}`, {
      method,
      headers,
      redirect: opts.redirect ?? "follow",
      ...(opts.body !== undefined ? { body: opts.body } : {})
    });
    return { status: response.status, body: await response.text().catch(() => "") };
  } catch (error) {
    return { status: 0, body: String(error) };
  }
}

/** The core invariant: bad input is a structured 4xx, never a 5xx. */
function expectNo5xx(label: string, r: Res): void {
  if (r.status >= 500) {
    findings.push(`${label} → ${r.status} ${r.body.slice(0, 200)}`);
  }
  expect(r.status, `${label} returned ${r.status} (${r.body.slice(0, 160)})`).toBeLessThan(500);
}
function expect4xx(label: string, r: Res): void {
  expectNo5xx(label, r);
  expect(r.status, `${label} returned ${r.status}`).toBeGreaterThanOrEqual(400);
}

// --- test-side crc32 (build adversarial self-routing tokens) -----------------

function crc32b36(input: string): string {
  let crc = 0xffffffff;
  for (const byte of Buffer.from(input, "utf8")) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return ((crc ^ 0xffffffff) >>> 0).toString(36);
}
function craftToken(plane: string, code: string, ws: string, secret: string, fixCrc: boolean): string {
  const body = ["aex", plane, code, ws, secret].join("_");
  return `${body}_${fixCrc ? crc32b36(body) : "deadbeef"}`;
}

describe("LIVE API adversarial fuzz", () => {
  afterAll(() => {
    if (findings.length > 0) {
      console.error(`[live-api-fuzz] ${findings.length} ROBUSTNESS FINDING(S):\n  ${findings.join("\n  ")}`);
    }
  });

  it("whoami: valid bearer → 200; missing → 401; garbage → 400/401/403 (auth enforced, never 200)", async () => {
    const ok = await call("GET", "/api/whoami", { token: TOKEN });
    expect(ok.status, ok.body.slice(0, 160)).toBe(200);
    expect((await call("GET", "/api/whoami", { token: null })).status).toBe(401);
    await fc.assert(
      fc.asyncProperty(fc.string({ minLength: 1, maxLength: 40 }), async (junk) => {
        const r = await call("GET", "/api/whoami", { token: junk });
        expectNo5xx(`whoami garbage-token`, r);
        // The API deliberately distinguishes a STRUCTURALLY-invalid token (400
        // `malformed_token`) from a well-formed but unauthorized one (401/403).
        // The invariant is auth-enforced: garbage NEVER authenticates (never 200).
        expect([400, 401, 403]).toContain(r.status);
      }),
      { numRuns: Math.min(SESSIONS, 30) }
    );
  });

  it("region-token routing: any crafted aex_* token → {308,400,401,403,451}, never 5xx", async () => {
    const ws = fc.string({ minLength: 4, maxLength: 26 }).map((s) => s.replace(/[^A-Za-z0-9]/g, "0") || "0000");
    await fc.assert(
      fc.asyncProperty(
        fc.constantFrom("dev", "prd", "xyz"),
        fc.constantFrom("euw2", "usw2", "apn1", "zzz9"),
        ws,
        fc.boolean(),
        async (plane, code, wsId, fixCrc) => {
          const token = craftToken(plane, code, wsId, "feedfacecafebeef", fixCrc);
          const r = await call("GET", "/api/whoami", { token, redirect: "manual" });
          expectNo5xx(`crafted-token ${plane}/${code}/crc:${fixCrc}`, r);
          // 400 = malformed_token (bad CRC / structure); 308 = region redirect;
          // 401/403 = unauthorized; 451 = residency. Never 5xx, never 200.
          expect([308, 400, 401, 403, 451]).toContain(r.status);
        }
      ),
      { numRuns: Math.min(SESSIONS, 40) }
    );
  });

  it("POST /api/sessions: malformed bodies are edge-rejected 4xx (never 5xx, never a dispatched run)", async () => {
    // Only bodies GUARANTEED rejected BEFORE any run dispatch on either plane.
    const invalidJson = fc
      .string({ minLength: 0, maxLength: 60 })
      .filter((s) => {
        try {
          const v = JSON.parse(s);
          return !(v && typeof v === "object" && !Array.isArray(v)); // keep non-object JSON + parse failures
        } catch {
          return true;
        }
      });
    await fc.assert(
      fc.asyncProperty(invalidJson, async (raw) => {
        const r = await call("POST", "/api/sessions", { token: TOKEN, body: raw });
        expect4xx(`POST /api/sessions invalid-json`, r);
      }),
      { numRuns: Math.min(SESSIONS, 40) }
    );

    // stdio MCP transport — server-side backstop reject (cheap, pre-dispatch).
    const stdioBody = JSON.stringify({
      submission: { model: "anthropic/claude-haiku-4-5", prompt: ["x"], mcpServers: [{ name: "s", url: "stdio://x", transport: "stdio" }] },
      secrets: {}
    });
    expect4xx("POST /api/sessions stdio-mcp", await call("POST", "/api/sessions", { token: TOKEN, body: stdioBody }));

    // SSRF / non-https webhook — pre-dispatch reject.
    await fc.assert(
      fc.asyncProperty(
        fc.constantFrom("http://example.com/h", "https://127.0.0.1/h", "https://10.0.0.1/h", "https://169.254.169.254/h", "https://user:pw@example.com/h", "not-a-url"),
        async (url) => {
          const body = JSON.stringify({ submission: { model: "anthropic/claude-haiku-4-5", prompt: ["x"] }, secrets: {}, webhook: { url } });
          const r = await call("POST", "/api/sessions", { token: TOKEN, body });
          expect4xx(`POST /api/sessions bad-webhook ${url}`, r);
        }
      ),
      { numRuns: Math.min(SESSIONS, 12) }
    );
  });

  it("POST /api/sessions: reject determinism — same malformed body ⇒ same status (no dup)", async () => {
    await fc.assert(
      fc.asyncProperty(fc.constantFrom("@@@", "[]", '"x"', "123", "{bad}"), async (raw) => {
        const a = await call("POST", "/api/sessions", { token: TOKEN, body: raw });
        const b = await call("POST", "/api/sessions", { token: TOKEN, body: raw });
        expect4xx("POST /api/sessions determinism", a);
        expect(b.status).toBe(a.status);
      }),
      { numRuns: 5 }
    );
  });

  it("read endpoints: adversarial session ids → 4xx/2xx, never 5xx", async () => {
    const sessionId = fc.oneof(
      fc.string({ minLength: 0, maxLength: 40 }),
      fc.constantFrom("../etc", "%2e%2e", "ses_'; DROP", "🔥", "a".repeat(300), "..%2f..%2f")
    );
    await fc.assert(
      fc.asyncProperty(sessionId, fc.constantFrom("", "/events", "/files"), async (id, suffix) => {
        const enc = encodeURIComponent(id);
        const r = await call("GET", `/api/sessions/${enc}${suffix}`, { token: TOKEN });
        expectNo5xx(`GET /api/sessions/<id>${suffix}`, r);
        expect(r.status).toBeLessThan(500);
      }),
      { numRuns: Math.min(SESSIONS, 40) }
    );
  });

  it("read endpoints: adversarial session-list / presign inputs → never 5xx", async () => {
    const qp = fc.record(
      {
        limit: fc.oneof(fc.integer({ min: -9999, max: 99999 }).map(String), fc.string({ maxLength: 8 })),
        status: fc.oneof(fc.constantFrom("succeeded", "bogus", ""), fc.string({ maxLength: 12 })),
        cursor: fc.string({ maxLength: 24 })
      },
      { requiredKeys: [] }
    );
    await fc.assert(
      fc.asyncProperty(qp, async (q) => {
        const search = new URLSearchParams(q as Record<string, string>).toString();
        const r = await call("GET", `/api/sessions?${search}`, { token: TOKEN });
        expectNo5xx("GET /api/sessions?<query>", r);
      }),
      { numRuns: Math.min(SESSIONS, 30) }
    );
    // presign with adversarial/garbage bodies — reject path only.
    await fc.assert(
      fc.asyncProperty(
        fc.oneof(fc.constant("{}"), fc.constant('{"hash":123}'), fc.constant('{"hash":"notahash"}'), fc.string({ maxLength: 30 })),
        async (raw) => {
          const r = await call("POST", "/api/assets/presign", { token: TOKEN, body: raw });
          expectNo5xx("POST /api/assets/presign garbage", r);
        }
      ),
      { numRuns: Math.min(SESSIONS, 20) }
    );
  });
});
