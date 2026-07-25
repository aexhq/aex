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
  pathMatches
} from "../src/testing/wire-conformance.js";

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

function client(handler: (url: URL, method: string) => Response): HttpClient {
  return new HttpClient({
    baseUrl: "https://dev-api.aex.dev",
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
      expect(formatWireConformanceReport(report)).toContain("VIOLATION sessions.get");
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
