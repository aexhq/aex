import { describe, expect, test } from "bun:test";

import {
  Aex,
  ROUTES,
  type AexTransport,
  type RouteId,
  type WireRequest,
  type WireResponse,
} from "../../src/index.js";

const KEY = `aex_wk_euw1_0100000000e008000000000001_0100000000e008000000000000_${"A".repeat(42)}A`;

class ScriptedTransport implements AexTransport {
  readonly requests: WireRequest[] = [];

  async execute<T>(request: WireRequest): Promise<WireResponse<T>> {
    this.requests.push(request);
    return { status: 200, headers: new Headers(), body: { id: "fixture" } as T };
  }
}

describe("resource routing", () => {
  test("maps generated resource methods to route ids, paths, headers, and canonical body bytes", async () => {
    const transport = new ScriptedTransport();
    const aex = new Aex({ apiKey: KEY, transport });

    await aex.sessions.sessionRunGet({ sessionId: "ses_1", runId: "run_1" });
    await aex.workspaces.workspaceGet({ workspaceId: "wsp_1" });
    await aex.organizations.organizationsList({ query: { limit: "2" } });
    await aex.apiKeys.apiKeyCreate({
      body: { scopes: ["sessions:write"], name: "ci" },
      idempotencyKey: "idk_1",
    });

    expect(transport.requests.map((request) => request.routeId)).toEqual([
      "session_run_get",
      "workspace_get",
      "organizations_list",
      "api_key_create",
    ]);
    expect(transport.requests[0]?.path).toBe("/api/sessions/ses_1/runs/run_1");
    expect(transport.requests[1]?.path).toBe("/api/workspaces/wsp_1");
    expect(transport.requests[2]?.path).toBe("/api/organizations?limit=2");
    expect(transport.requests[3]?.headers.get("Idempotency-Key")).toBe("idk_1");
    expect(new TextDecoder().decode(transport.requests[3]?.body)).toBe(
      '{"name":"ci","scopes":["sessions:write"]}',
    );
  });

  test("publishes no resource method for an operation the platform does not serve", () => {
    // The server is the only authority on what it serves, so the SDK never
    // refuses a call locally. What it does not do is offer a typed method that
    // could only ever return `501 not_implemented`.
    const sessions = aexSessionsSurface();
    expect(sessions).toContain("sessionRunGet");
    for (const method of ["sessionCreate", "sessionGet", "sessionStop", "sessionTrash"]) {
      expect({ method, published: sessions.includes(method) }).toEqual({ method, published: false });
    }
  });

  test("execute stays total over every route id, deferred operations included", async () => {
    const transport = new ScriptedTransport();
    const aex = new Aex({ apiKey: KEY, transport });
    const deferred = (Object.keys(ROUTES) as RouteId[]).filter((id) => ROUTES[id].deferred);
    expect(deferred.length).toBeGreaterThan(0);

    const id = deferred[0] as RouteId;
    const bindings = Object.fromEntries(ROUTES[id].pathParams.map((name) => [name, "fixture"]));
    await aex.execute(id, bindings);
    expect(transport.requests[0]?.routeId).toBe(id);
  });
});

function aexSessionsSurface(): string[] {
  const aex = new Aex({ apiKey: KEY, transport: new ScriptedTransport() });
  return Object.getOwnPropertyNames(Object.getPrototypeOf(aex.sessions));
}
