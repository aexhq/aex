import { describe, expect, it } from "bun:test";
import { newId, type FetchLike } from "@aexhq/contracts";
import { Aex } from "../../src/index.js";

const BOOTSTRAP = "https://api.aex.test";
const REGIONAL = "https://eu-west-1.api.aex.test";
const OTHER_REGIONAL = "https://us-east-1.api.aex.test";
const ORGID = newId("organization");
const WID = newId("workspace");
const WID2 = newId("workspace");
const KEYID = newId("apiKey");
const STMID = newId("statement");
const OPID = newId("operation");
const at = "2026-07-30T10:00:00.000Z";
const later = "2026-07-31T10:00:00.000Z";
const hash = `sha256:${"a".repeat(64)}`;

type Call = {
  readonly url: URL;
  readonly method: string;
  readonly headers: Headers;
  readonly body: unknown;
};

function json(value: unknown, status = 200, headers: HeadersInit = {}): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json", ...headers }
  });
}

function recordingClient(
  respond: (call: Call) => Response | Promise<Response>
): { readonly client: Aex; readonly calls: Call[] } {
  const calls: Call[] = [];
  const fetch: FetchLike = async (input, init) => {
    const headers = new Headers(init?.headers);
    const call: Call = {
      url: new URL(input instanceof URL ? input.href : String(input)),
      method: init?.method ?? "GET",
      headers,
      body: typeof init?.body === "string" &&
          headers.get("content-type") === "application/json"
        ? JSON.parse(init.body)
        : init?.body
    };
    calls.push(call);
    return respond(call);
  };
  return {
    client: new Aex({
      apiKey: "account-token",
      bootstrapBaseUrl: BOOTSTRAP,
      baseUrl: REGIONAL,
      fetch,
      retry: false
    }),
    calls
  };
}

const activeState = { status: "active", revision: 2, changedAt: at } as const;

function workspace(id: string, region = "eu-west-1", apiUrl = REGIONAL) {
  return {
    id,
    organizationId: ORGID,
    name: `Workspace ${id.slice(-4)}`,
    slug: `workspace-${id.slice(-4)}`,
    region,
    apiUrl,
    status: "active",
    operationalState: {
      ...activeState,
      inheritedFrom: "account",
      organizationId: ORGID
    },
    createdAt: at
  };
}

describe("v1 bootstrap account, organizations, workspaces, and keys", () => {
  it("constructs without implicit first-sign-in mutations", () => {
    const { calls } = recordingClient(() => json({}));
    expect(calls).toEqual([]);
  });

  it("routes explicit organization, membership, invitation, and workspace creation", async () => {
    const organization = {
      id: ORGID,
      name: "Example",
      slug: "example",
      callerRole: "owner",
      createdAt: at
    };
    const { client, calls } = recordingClient((call) => {
      if (call.url.pathname.endsWith("/memberships")) return json({ items: [] });
      if (call.url.pathname.endsWith("/invitations")) {
        return json({
          id: newId("invitation"),
          organizationId: ORGID,
          email: "member@example.com",
          role: "member",
          status: "pending",
          createdAt: at,
          expiresAt: later
        }, 201);
      }
      if (call.url.pathname === "/api/workspaces" && call.method === "POST") {
        return json(workspace(WID), 201);
      }
      if (call.url.pathname === "/api/workspaces") {
        return json({ items: [workspace(WID)] });
      }
      return json(organization, call.method === "POST" ? 201 : 200);
    });

    await client.account.get({ organizationId: ORGID });
    await client.organizations.list();
    await client.organizations.create({ name: "Example" });
    await client.organizations.get(ORGID);
    await client.organizations.memberships.list(ORGID);
    await client.organizations.invitations.create(ORGID, {
      email: "member@example.com",
      role: "member"
    });
    await client.workspaces.create({
      organizationId: ORGID,
      name: "Production",
      region: "eu-west-1"
    });
    await client.workspaces.list({ organizationId: ORGID });

    expect(calls.every(({ url }) => url.origin === BOOTSTRAP)).toBe(true);
    expect(calls[0]!.url.searchParams.get("organizationId")).toBe(ORGID);
    expect(calls[2]!.headers.get("idempotency-key")).toBeTruthy();
    expect(calls[5]!.headers.get("idempotency-key")).toBeTruthy();
    expect(calls[6]!.body).toEqual({
      organizationId: ORGID,
      name: "Production",
      region: "eu-west-1"
    });
  });

  it("separates one-time key minting and durable workspace deletion identities", async () => {
    const apiKey = {
      id: KEYID,
      workspaceId: WID,
      name: "automation",
      scopes: ["sessions:write"],
      createdAt: at,
      revokedAt: null
    };
    const value = `aex_wk_euw1_${KEYID.slice(4)}_${"A".repeat(43)}`;
    const { client, calls } = recordingClient((call) => {
      if (call.url.pathname === "/api/api-keys" && call.method === "GET") {
        return json({ items: [apiKey] });
      }
      if (call.url.pathname === "/api/api-keys") {
        return json({ ...apiKey, value }, 201);
      }
      if (call.url.pathname.includes("/deletions")) {
        return json({
          id: OPID,
          workspaceId: WID,
          kind: "workspace_delete",
          status: "queued",
          cancelable: false,
          createdAt: at,
          updatedAt: at
        }, 202);
      }
      return new Response(null, { status: 204 });
    });

    await client.apiKeys.list({ workspaceId: WID });
    const created = await client.apiKeys.create({
      workspaceId: WID,
      name: "automation",
      scopes: ["sessions:write"]
    });
    await client.apiKeys.revoke(KEYID, { ifRevision: 2 });
    const deletion = await client.workspaces.delete(WID, {
      confirmation: WID,
      operationId: OPID
    });

    expect(created.value).toBe(value);
    expect(calls[0]!.url.searchParams.get("workspaceId")).toBe(WID);
    expect(calls[1]!.headers.get("idempotency-key")).toBeTruthy();
    expect(calls[2]!.headers.get("if-match")).toBe("\"2\"");
    expect(calls[3]!.headers.get("aex-operation-id")).toBe(OPID);
    expect(calls[3]!.headers.get("idempotency-key")).toBeNull();
    expect(calls[3]!.body).toEqual({ confirmation: WID });
    expect(deletion.kind).toBe("workspace_delete");
  });

  it("reads bootstrap placement and the regional inherited workspace state", async () => {
    const record = workspace(WID);
    const { client, calls } = recordingClient(() => json(record));

    await client.workspaces.get(WID);
    await client.workspace.get();

    expect(calls.map(({ url }) => [url.origin, url.pathname])).toEqual([
      [BOOTSTRAP, `/api/workspaces/${WID}`],
      [REGIONAL, "/api/workspace"]
    ]);
  });
});

describe("v1 bootstrap billing and immutable statements", () => {
  it("reads balance and fully replaces revisioned auto-topup policy", async () => {
    const policy = {
      enabled: false,
      thresholdUsd: 5,
      amountUsd: 20,
      revision: 2,
      updatedAt: at
    };
    const { client, calls } = recordingClient((call) => {
      if (call.url.pathname === "/api/billing/balance") {
        return json({
          organizationId: ORGID,
          currency: "USD",
          revision: 2,
          availableCents: "1200",
          reservedCents: "200",
          pendingCents: "0",
          operationalState: activeState,
          updatedAt: at
        });
      }
      return json(policy, 200, { etag: "\"2\"" });
    });

    await client.billing.balance.get({ organizationId: ORGID });
    await client.billing.autoTopup.get(ORGID);
    await client.billing.autoTopup.replace(ORGID, {
      enabled: false,
      thresholdUsd: 5,
      amountUsd: 20
    }, { ifRevision: 2 });

    expect(calls[0]!.url.searchParams.get("organizationId")).toBe(ORGID);
    expect(calls[2]!.method).toBe("PUT");
    expect(calls[2]!.headers.get("if-match")).toBe("\"2\"");
    expect(calls[2]!.headers.get("idempotency-key")).toBeTruthy();
    expect(calls[2]!.body).toEqual({
      enabled: false,
      thresholdUsd: 5,
      amountUsd: 20
    });
  });

  it("creates explicit checkout/portal sessions without replaying paused work", async () => {
    const { client, calls } = recordingClient(() =>
      json({ url: "https://billing.example.test", expiresAt: later }));

    await client.billing.topUpCheckout(ORGID, { amountUsd: 25 });
    await client.billing.portalSession(ORGID, { returnUrl: "https://app.example.test" });

    expect(calls.map(({ url }) => url.pathname)).toEqual([
      `/api/organizations/${ORGID}/billing/top-up-checkouts`,
      `/api/organizations/${ORGID}/billing/portal-sessions`
    ]);
    expect(calls.every(({ headers }) => headers.has("idempotency-key"))).toBe(true);
  });

  it("lists/gets immutable statements and explicitly mints a download grant", async () => {
    const statement = {
      id: STMID,
      organizationId: ORGID,
      period: { gte: at, lt: later },
      currency: "USD",
      totalCents: "325",
      lines: [{ category: "compute", totalCents: "325" }],
      artifactHash: hash,
      issuedAt: later
    };
    const { client, calls } = recordingClient((call) =>
      call.url.pathname.endsWith("/downloads")
        ? json({
            url: "https://download.example.test/statement",
            expiresAt: later,
            sizeBytes: 12,
            authorizedBytes: 12,
            measurementId: newId("measurement"),
            sha256: hash
          })
        : call.url.pathname.endsWith(`/${STMID}`)
          ? json(statement)
          : json({ items: [statement] }));

    await client.billing.statements.list(ORGID);
    await client.billing.statements.get(ORGID, STMID);
    await client.billing.statements.download(ORGID, STMID);

    expect(calls[2]!.method).toBe("POST");
    expect(calls[2]!.headers.get("idempotency-key")).toBeTruthy();
  });
});

describe("v1 regional usage and explicit organization merge", () => {
  it("queries one regional workspace without mutation identity headers", async () => {
    const { client, calls } = recordingClient(() =>
      json({ items: [], frontiers: [] }));
    await client.billing.usage.query({
      timeRange: { gte: at, lt: later },
      categories: ["storage"]
    });

    expect(calls[0]!.url.origin).toBe(REGIONAL);
    expect(calls[0]!.url.pathname).toBe("/api/billing/usage/query");
    expect(calls[0]!.headers.get("idempotency-key")).toBeNull();
    expect(calls[0]!.headers.get("aex-operation-id")).toBeNull();
  });

  it("enumerates bootstrap workspaces and merges regional pages without proxying", async () => {
    const { client, calls } = recordingClient((call) => {
      if (call.url.origin === BOOTSTRAP) {
        return json({
          items: [
            workspace(WID),
            workspace(WID2, "us-east-1", OTHER_REGIONAL)
          ]
        });
      }
      const workspaceId = call.url.searchParams.get("workspaceId")!;
      const region = call.url.origin === REGIONAL ? "eu-west-1" : "us-east-1";
      return json({
        items: [{
          category: "storage",
          region,
          workspaceId,
          source: "persisted_file",
          ratedCents: "1",
          serviceTime: { gte: at, lt: later },
          quantity: { kind: "byte_milliseconds", byteMilliseconds: "1000" }
        }],
        frontiers: [{
          region,
          workspaceId,
          category: "storage",
          acceptedSequence: "4",
          ratedSequence: "4",
          aggregatedSequence: "4",
          settledSequence: "4",
          serviceThrough: later
        }]
      });
    });

    const merged = await client.billing.usage.queryOrganization(ORGID, {
      timeRange: { gte: at, lt: later },
      categories: ["storage"]
    });

    expect(merged.items).toHaveLength(2);
    expect(merged.frontiers).toHaveLength(2);
    expect(calls.map(({ url }) => url.origin)).toEqual([
      BOOTSTRAP,
      REGIONAL,
      OTHER_REGIONAL
    ]);
    expect(calls.slice(1).every(({ url }) =>
      url.pathname === "/api/billing/usage/query" &&
      url.searchParams.has("workspaceId"))).toBe(true);
  });

  it("does not expose removed billing/control aliases", async () => {
    const root = await import("../../src/index.js") as Record<string, unknown>;
    for (const name of [
      "PlansClient",
      "SubscriptionsClient",
      "BillingAllowance",
      "BillingTopup",
      "AccountPatClient",
      "NewWorkspaceWithKey"
    ]) {
      expect(root[name]).toBeUndefined();
    }
  });
});
