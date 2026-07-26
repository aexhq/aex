/**
 * C4 — the binding table and the harness that consumes it.
 *
 * Two things are pinned here, and the second is the one that makes a green run
 * mean something:
 *
 * 1. **The seam works end to end.** A response really does reach the harness
 *    through `HttpClient`, at the path the bindings expect. This is what proves
 *    the `/api` prefix: `api-routes.ts` declares `/sessions` and the client
 *    requests `/api/sessions`, and if those two ever disagree every route silently
 *    becomes "no schema" — a harness reporting zero violations because it checked
 *    nothing.
 * 2. **Coverage is accounted for, not assumed.** Every route in the table is
 *    either bound or explicitly excused; nothing falls between.
 */
import { describe, expect, it } from "bun:test";
import { AUTHENTICATED_API_ROUTE_DESCRIPTORS } from "../src/api-routes.js";
import { HttpClient } from "../src/http.js";
import {
  DATA_PLANE_RESPONSE_SCHEMAS,
  ROUTES_OFF_THE_SDK_SEAM,
  ROUTES_WITHOUT_RESPONSE_SCHEMA,
  formatResponseSchemaCoverage,
  routeMatchTemplate
} from "../src/testing/response-bindings.js";
import {
  formatWireConformanceReport,
  installWireConformance,
  mergeWireConformanceReports,
  pathMatches,
  wireOrigin
} from "../src/testing/wire-conformance.js";
import { ApiErrorEnvelopeSchema } from "../src/schemas/response-common.js";
import dataPlaneDocument from "../openapi/data-plane.json" with { type: "json" };

const TS = "2026-07-25T12:00:00.000Z";

const sessionWire = {
  id: "sess_1",
  status: "idle",
  acceptsMessages: true,
  runtimeSize: "0.25cpu-1gb",
  runtimeKind: "lambda",
  createdAt: TS,
  updatedAt: TS,
  dataState: "active"
};

const DATA_PLANE = "https://dev-api.aex.dev";
const CONTROL_PLANE = "https://aex.dev";

function client(
  handler: (url: URL, method: string) => Response,
  baseUrl = DATA_PLANE
): HttpClient {
  return new HttpClient({
    baseUrl,
    apiKey: "aex_test_key",
    fetch: async (input, init) =>
      handler(input instanceof URL ? input : new URL(String(input)), (init?.method ?? "GET").toUpperCase())
  });
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" }
  });
}

describe("route match templates", () => {
  it("collapses every variable segment to a placeholder", () => {
    expect(routeMatchTemplate(/^\/sessions$/)).toBe("/sessions");
    expect(routeMatchTemplate(/^\/sessions\/[^/]+\/messages$/)).toBe("/sessions/{param}/messages");
    expect(routeMatchTemplate(/^\/sessions\/[^/]+\/files\/[^/]+\/link$/)).toBe(
      "/sessions/{param}/files/{param}/link"
    );
    expect(routeMatchTemplate(/^\/assets\/[^/]+$/)).toBe("/assets/{param}");
  });

  it("never leaves a character class fragment in a template", () => {
    for (const binding of DATA_PLANE_RESPONSE_SCHEMAS) {
      expect(binding.path).not.toContain("[");
      expect(binding.path).not.toContain("\\");
      expect(binding.path.startsWith("/api/")).toBe(true);
    }
  });

  it("matches the route table's own sample path for every binding", () => {
    const byName = new Map(AUTHENTICATED_API_ROUTE_DESCRIPTORS.map((route) => [route.name, route]));
    for (const binding of DATA_PLANE_RESPONSE_SCHEMAS) {
      const descriptor = byName.get(binding.name);
      expect(descriptor).toBeDefined();
      expect(pathMatches(binding.path, `/api${descriptor!.samplePath}`)).toBe(true);
      expect(binding.method).toBe(descriptor!.method.toUpperCase());
    }
  });
});

describe("coverage accounting", () => {
  it("accounts for every data-plane route exactly once", () => {
    const bound = new Set(DATA_PLANE_RESPONSE_SCHEMAS.map((binding) => binding.name));
    const excused = new Set(ROUTES_WITHOUT_RESPONSE_SCHEMA.map((route) => route.name));
    const unaccounted = AUTHENTICATED_API_ROUTE_DESCRIPTORS.map((route) => route.name).filter(
      (name) => !bound.has(name) && !excused.has(name)
    );
    expect(unaccounted).toEqual([]);
    for (const name of excused) {
      expect(bound.has(name)).toBe(false);
    }
  });

  it("excuses only routes that exist", () => {
    const known = new Set(AUTHENTICATED_API_ROUTE_DESCRIPTORS.map((route) => route.name));
    for (const route of [...ROUTES_WITHOUT_RESPONSE_SCHEMA, ...ROUTES_OFF_THE_SDK_SEAM]) {
      expect(known.has(route.name)).toBe(true);
      expect(route.reason.length).toBeGreaterThan(20);
    }
  });

  it("only calls a BOUND route off-seam — an unbound route is already excused", () => {
    const bound = new Set(DATA_PLANE_RESPONSE_SCHEMAS.map((binding) => binding.name));
    for (const route of ROUTES_OFF_THE_SDK_SEAM) {
      expect(bound.has(route.name)).toBe(true);
    }
  });

  it("binds no two schemas to the same method and path", () => {
    const seen = new Set<string>();
    for (const binding of DATA_PLANE_RESPONSE_SCHEMAS) {
      const key = `${binding.method} ${binding.path}`;
      expect(seen.has(key)).toBe(false);
      seen.add(key);
    }
  });

  it("prints both the excused routes and the unreachable ones", () => {
    const rendered = formatResponseSchemaCoverage();
    expect(rendered).toContain(
      `${DATA_PLANE_RESPONSE_SCHEMAS.length}/${AUTHENTICATED_API_ROUTE_DESCRIPTORS.length}`
    );
    for (const route of ROUTES_WITHOUT_RESPONSE_SCHEMA) {
      expect(rendered).toContain(route.name);
    }
    for (const route of ROUTES_OFF_THE_SDK_SEAM) {
      expect(rendered).toContain(route.name);
    }
  });
});

describe("the harness over a real HttpClient", () => {
  it("validates a response that arrives through the client", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS);
    try {
      const http = client(() => jsonResponse({ session: sessionWire }));
      await http.request("/api/sessions/sess_1");
      const report = harness.report();
      expect(report.observed).toBe(1);
      expect(report.validated).toEqual(["sessions.get"]);
      expect(report.violations).toEqual([]);
      expect(report.unschemad).toEqual([]);
    } finally {
      harness.stop();
    }
  });

  it("reports a violation with the operation, the path and the offending field", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS);
    try {
      const http = client(() =>
        jsonResponse({ session: { ...sessionWire, runtime: { kind: "lambda" } } })
      );
      await http.request("/api/sessions/sess_1");
      const report = harness.report();
      expect(report.violations).toHaveLength(1);
      const violation = report.violations[0]!;
      expect(violation.name).toBe("sessions.get");
      expect(violation.method).toBe("GET");
      expect(violation.path).toBe("/api/sessions/sess_1");
      expect(violation.status).toBe(200);
      expect(violation.issues.join(" ")).toContain("runtime");
      expect(violation.kind).toBe("response");
      // The BODY travels with the complaint: a reader has to be able to tell a
      // server defect from a schema defect, and only the bytes settle that.
      expect(violation.body).toEqual({
        session: { ...sessionWire, runtime: { kind: "lambda" } }
      });
      const rendered = formatWireConformanceReport(report);
      expect(rendered).toContain("VIOLATION [response] sessions.get");
      expect(rendered).toContain('"runtime":{"kind":"lambda"}');
    } finally {
      harness.stop();
    }
  });

  it("records a response with no binding as unschemad rather than as a pass", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS);
    try {
      // A CONTROL-plane path. It is not in the data-plane route table, so the
      // honest outcome is "nothing checked this", never a silent success.
      const http = client(() => jsonResponse({ orgs: [] }));
      await http.request("/api/orgs");
      const report = harness.report();
      expect(report.validated).toEqual([]);
      expect(report.unschemad).toEqual(["GET /api/orgs"]);
      expect(formatWireConformanceReport(report)).toContain("NO SCHEMA");
    } finally {
      harness.stop();
    }
  });

  it("treats an empty 204 body as the declared empty object", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS);
    try {
      const http = client(() => new Response("", { status: 204 }));
      await http.request("/api/secrets/API_KEY", { method: "DELETE" });
      const report = harness.report();
      expect(report.validated).toEqual(["secrets.delete"]);
      expect(report.violations).toEqual([]);
    } finally {
      harness.stop();
    }
  });

  it("distinguishes two operations that share a path but not a method", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS);
    try {
      const http = client((_url, method) =>
        method === "GET"
          ? jsonResponse({ sessions: [sessionWire] })
          : jsonResponse({ session: sessionWire }, 201)
      );
      await http.request("/api/sessions");
      await http.request("/api/sessions", { method: "POST", body: "{}" });
      const report = harness.report();
      expect([...report.validated].sort()).toEqual(["sessions.create", "sessions.list"]);
      expect(report.violations).toEqual([]);
    } finally {
      harness.stop();
    }
  });

  it("always names the routes it did not exercise", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS);
    try {
      const http = client(() => jsonResponse({ session: sessionWire }));
      await http.request("/api/sessions/sess_1");
      const rendered = formatWireConformanceReport(harness.report());
      expect(rendered).toContain("NOT EXERCISED");
      expect(rendered).toContain("billing.get");
      expect(rendered).toContain("1 operation(s) validated");
    } finally {
      harness.stop();
    }
  });

  it("stops observing once the harness is stopped", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS);
    harness.stop();
    const http = client(() => jsonResponse({ session: sessionWire }));
    await http.request("/api/sessions/sess_1");
    expect(harness.report().observed).toBe(0);
  });
});

/**
 * The plane collision — the reason this table could not previously be installed
 * anywhere that drives both planes.
 */
describe("origin", () => {
  const accountWhoAmI = {
    ok: true,
    principalType: "account_token",
    appUserId: "user_1",
    scopes: []
  };

  it("reads an origin off any URL on the plane", () => {
    expect(wireOrigin(DATA_PLANE)).toBe(DATA_PLANE);
    expect(wireOrigin(`${DATA_PLANE}/`)).toBe(DATA_PLANE);
    expect(wireOrigin(`${DATA_PLANE}/api/sessions?x=1`)).toBe(DATA_PLANE);
    expect(wireOrigin(CONTROL_PLANE)).not.toBe(DATA_PLANE);
  });

  it("refuses an unreadable base URL instead of quietly matching everything", () => {
    expect(() => wireOrigin("not a url")).toThrow(/could not read an origin/);
  });

  it("WITHOUT an origin, a control-plane whoami is judged by a data-plane schema", async () => {
    // This is the false violation the collision produces, pinned so the fix
    // cannot be undone silently.
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS);
    try {
      const http = client(() => jsonResponse(accountWhoAmI), CONTROL_PLANE);
      await http.request("/api/whoami");
      const report = harness.report();
      expect(report.violations).toHaveLength(1);
      expect(report.violations[0]!.name).toBe("whoami");
      expect(formatWireConformanceReport(report)).toContain("ORIGIN FILTER: none");
    } finally {
      harness.stop();
    }
  });

  it("WITH an origin, the same response is reported off-plane and not judged", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: DATA_PLANE });
    try {
      const http = client(() => jsonResponse(accountWhoAmI), CONTROL_PLANE);
      await http.request("/api/whoami");
      const report = harness.report();
      expect(report.violations).toEqual([]);
      expect(report.validated).toEqual([]);
      expect(report.unschemad).toEqual([]);
      expect(report.offPlane).toEqual([`${CONTROL_PLANE} GET /api/whoami`]);
      expect(report.observed).toBe(1);
      const rendered = formatWireConformanceReport(report);
      expect(rendered).toContain(`ORIGIN FILTER: ${DATA_PLANE}`);
      expect(rendered).toContain("OFF-PLANE");
    } finally {
      harness.stop();
    }
  });

  it("still validates the SAME path when it comes from the plane the bindings describe", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: `${DATA_PLANE}/` });
    try {
      const http = client(() => jsonResponse(accountWhoAmI), DATA_PLANE);
      await http.request("/api/whoami");
      const report = harness.report();
      expect(report.offPlane).toEqual([]);
      expect(report.violations).toHaveLength(1);
      expect(report.violations[0]!.name).toBe("whoami");
      expect(report.violations[0]!.origin).toBe(DATA_PLANE);
    } finally {
      harness.stop();
    }
  });
});

/**
 * The 2xx-only gap. Every generated operation declares an error envelope, and
 * before `HttpClient` reported ahead of its throw, none of those 68 declarations
 * was ever checked against a byte.
 */
describe("error envelopes", () => {
  it("checks a non-2xx body the client is about to throw on", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: DATA_PLANE });
    try {
      const http = client(() =>
        jsonResponse({ error: "not_found", message: "no such session", requestId: "req_1" }, 404)
      );
      await expect(http.request("/api/sessions/sess_1")).rejects.toThrow();
      const report = harness.report();
      expect(report.observed).toBe(1);
      expect(report.violations).toEqual([]);
      expect(report.errorsValidated).toEqual(["GET /api/sessions/sess_1 -> 404"]);
      // A 404 is NOT evidence that the 2xx schema was exercised.
      expect(report.validated).toEqual([]);
      expect(report.unexercised).toContain("sessions.get");
      expect(formatWireConformanceReport(report)).toContain("ERROR ENVELOPES CHECKED");
    } finally {
      harness.stop();
    }
  });

  it("accepts the extra fields individual codes carry", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: DATA_PLANE });
    try {
      const http = client(() =>
        jsonResponse(
          { error: "session_busy", message: "a turn is in flight", status: "running" },
          409
        )
      );
      await expect(
        http.request("/api/sessions/sess_1/messages", { method: "POST", body: "{}" })
      ).rejects.toThrow();
      expect(harness.report().violations).toEqual([]);
    } finally {
      harness.stop();
    }
  });

  it("fails an error body that carries no stable code", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: DATA_PLANE });
    try {
      // What an API-Gateway-native rejection looks like: it never reaches the
      // handler that would have built the envelope.
      const http = client(() => jsonResponse({ message: "Forbidden" }, 403));
      await expect(http.request("/api/sessions")).rejects.toThrow();
      const report = harness.report();
      expect(report.violations).toHaveLength(1);
      const violation = report.violations[0]!;
      expect(violation.kind).toBe("error-envelope");
      expect(violation.status).toBe(403);
      expect(violation.issues.join(" ")).toContain("error");
      expect(violation.body).toEqual({ message: "Forbidden" });
      expect(formatWireConformanceReport(report)).toContain("VIOLATION [error-envelope]");
    } finally {
      harness.stop();
    }
  });

  it("covers the error side of routes that have NO 2xx schema", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: DATA_PLANE });
    try {
      const http = client(() => jsonResponse({ error: "not_found", message: "gone" }, 404));
      await expect(
        http.download("/api/sessions/sess_1/files/out.txt")
      ).rejects.toThrow();
      const report = harness.report();
      expect(report.errorsValidated).toEqual(["GET /api/sessions/sess_1/files/out.txt -> 404"]);
      expect(report.violations).toEqual([]);
    } finally {
      harness.stop();
    }
  });

  it("reports the RAW body, not the client's requestId enrichment", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: DATA_PLANE });
    try {
      const http = client(
        () =>
          new Response(JSON.stringify({ error: "internal_error", message: "boom" }), {
            status: 500,
            headers: { "content-type": "application/json", "x-request-id": "req_from_header" }
          })
      );
      await expect(http.request("/api/sessions")).rejects.toThrow();
      // `HttpClient` folds x-request-id into the error it throws. Validating THAT
      // object would be checking our own client, not the server's bytes.
      expect(harness.report().violations).toEqual([]);
      const observedBody = harness.report().errorsValidated;
      expect(observedBody).toEqual(["GET /api/sessions -> 500"]);
    } finally {
      harness.stop();
    }
  });

  it("names a redirect rather than judging its empty body as an envelope", async () => {
    const harness = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: DATA_PLANE });
    try {
      const http = client(
        () => new Response("", { status: 302, headers: { location: "https://storage.example/x" } })
      );
      await expect(http.download("/api/sessions/sess_1/archive")).rejects.toThrow();
      const report = harness.report();
      expect(report.violations).toEqual([]);
      expect(report.errorsValidated).toEqual([]);
      expect(report.unschemad.join(" ")).toContain("302 (redirect — no declared body)");
    } finally {
      harness.stop();
    }
  });

  it("agrees with the component the generated document declares", () => {
    // Two declarations of one envelope exist: this schema, and the literal the
    // OpenAPI generator writes into `components.schemas`. Pin them together so
    // the pair cannot drift unnoticed.
    const component = dataPlaneDocument.components.schemas.ApiErrorEnvelope;
    expect([...component.required].sort()).toEqual(["error", "message"]);
    expect(component.additionalProperties).toBe(true);
    // Required in the document == rejected by the schema when absent.
    for (const missing of ["error", "message"]) {
      const body: Record<string, string> = { error: "x", message: "y" };
      delete body[missing];
      expect(ApiErrorEnvelopeSchema["~standard"].validate(body)).toHaveProperty("issues");
    }
    // Open in the document == accepted by the schema when extended.
    expect(
      ApiErrorEnvelopeSchema["~standard"].validate({ error: "x", message: "y", extra: 1 })
    ).not.toHaveProperty("issues");
  });
});

describe("merging reports across processes", () => {
  it("unions what was seen and intersects what was not", async () => {
    const first = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: DATA_PLANE });
    await client(() => jsonResponse({ session: sessionWire })).request("/api/sessions/sess_1");
    const firstReport = first.report();
    first.stop();

    const second = installWireConformance(DATA_PLANE_RESPONSE_SCHEMAS, { origin: DATA_PLANE });
    await client(() => jsonResponse({ sessions: [sessionWire] })).request("/api/sessions");
    const secondReport = second.report();
    second.stop();

    const merged = mergeWireConformanceReports([firstReport, secondReport]);
    expect(merged.observed).toBe(2);
    expect(merged.validated).toEqual(["sessions.get", "sessions.list"]);
    expect(merged.originFilter).toBe(DATA_PLANE);
    // Neither run exercised billing; both exercised something the other did not.
    expect(merged.unexercised).toContain("billing.get");
    expect(merged.unexercised).not.toContain("sessions.get");
    expect(merged.unexercised).not.toContain("sessions.list");
  });

  it("does not claim an origin filter every fragment did not apply", () => {
    const filtered: Parameters<typeof mergeWireConformanceReports>[0][number] = {
      originFilter: DATA_PLANE,
      validated: [],
      unexercised: [],
      unschemad: [],
      errorsValidated: [],
      offPlane: [],
      violations: [],
      observed: 0
    };
    expect(mergeWireConformanceReports([filtered, { ...filtered, originFilter: undefined }]).originFilter).toBeUndefined();
    expect(mergeWireConformanceReports([filtered, filtered]).originFilter).toBe(DATA_PLANE);
  });

  it("is empty, not undefined, over no fragments at all", () => {
    const merged = mergeWireConformanceReports([]);
    expect(merged.observed).toBe(0);
    expect(merged.validated).toEqual([]);
    expect(merged.unexercised).toEqual([]);
  });
});
