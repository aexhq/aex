import { describe, expect, test } from "bun:test";

import {
  Aex,
  ROUTES,
  type AexTransport,
  type RouteId,
  type WireRequest,
  type WireResponse,
} from "../../src/index.js";
import packageManifest from "../../package.json" with { type: "json" };

const KEY = `aex_wk_euw1_0100000000e008000000000001_0100000000e008000000000000_${"A".repeat(42)}A`;

class ScriptedTransport implements AexTransport {
  readonly requests: WireRequest[] = [];

  async execute<T>(request: WireRequest): Promise<WireResponse<T>> {
    this.requests.push(request);
    return { status: 200, headers: new Headers(), body: { id: "fixture" } as T };
  }
}

describe("resource routing", () => {
  test("reports the exact package manifest version in build metadata and requests", async () => {
    const transport = new ScriptedTransport();
    const aex = new Aex({ apiKey: KEY, transport });

    await aex.organizations.organizationsList();

    expect(Aex.buildInfo().packageVersion).toBe(packageManifest.version);
    expect(transport.requests[0]?.headers.get("Aex-Client")).toBe(
      `aex-sdk/${packageManifest.version}`,
    );
  });

  test("maps generated resource methods to route ids, paths, headers, and canonical body bytes", async () => {
    const transport = new ScriptedTransport();
    const aex = new Aex({ apiKey: KEY, transport });

    await aex.sessions.sessionGet({ sessionId: "ses_1" });
    await aex.workspaces.workspaceGet({ workspaceId: "wsp_1" });
    await aex.organizations.organizationsList({ query: { limit: "2" } });
    await aex.apiKeys.apiKeyCreate({
      body: { scopes: ["sessions:write"], name: "ci", workspaceId: "wsp_1" },
      idempotencyKey: "idk_1",
    });

    expect(transport.requests.map((request) => request.routeId)).toEqual([
      "session_get",
      "workspace_get",
      "organizations_list",
      "api_key_create",
    ]);
    expect(transport.requests[0]?.path).toBe("/api/sessions/ses_1");
    expect(transport.requests[1]?.path).toBe("/api/workspaces/wsp_1");
    expect(transport.requests[2]?.path).toBe("/api/organizations?limit=2");
    expect(transport.requests[3]?.headers.get("Idempotency-Key")).toBe("idk_1");
    expect(new TextDecoder().decode(transport.requests[3]?.body)).toBe(
      '{"name":"ci","scopes":["sessions:write"],"workspaceId":"wsp_1"}',
    );
  });

  test("publishes no resource method for an operation the platform does not serve", () => {
    // The server is the only authority on what it serves, so the SDK never
    // refuses a call locally. What it does not do is offer a typed method that
    // could only ever return `501 not_implemented`.
    //
    // The deferred set is read from the route registry rather than listed
    // here. A hand-typed list goes stale the moment a lane mounts one of its
    // entries, and then it quietly asserts the opposite of what its name says.
    const sessions = aexSessionsSurface();
    expect(sessions).toContain("sessionGet");
    expect(sessions).not.toContain("sessionRunGet");
    const deferred = (Object.keys(ROUTES) as RouteId[])
      .filter((id) => ROUTES[id].deferred)
      .map(resourceMethodName);
    for (const method of deferred) {
      expect({ method, published: sessions.includes(method) }).toEqual({ method, published: false });
    }
  });

  test("execute stays total over every route id, deferred operations included", async () => {
    const transport = new ScriptedTransport();
    const aex = new Aex({ apiKey: KEY, transport });
    const deferred = (Object.keys(ROUTES) as RouteId[]).filter((id) => ROUTES[id].deferred);
    const id = deferred[0] ?? "account_get";
    const bindings = Object.fromEntries(ROUTES[id].pathParams.map((name) => [name, "fixture"]));
    await aex.execute(id, bindings);
    expect(transport.requests[0]?.routeId).toBe(id);
  });
});

/// The resource method a route id is published as, if it is published at all.
function resourceMethodName(id: string): string {
  return id.replace(/_([a-z0-9])/g, (_match, character: string) => character.toUpperCase());
}

function aexSessionsSurface(): string[] {
  const aex = new Aex({ apiKey: KEY, transport: new ScriptedTransport() });
  return Object.getOwnPropertyNames(Object.getPrototypeOf(aex.sessions));
}
