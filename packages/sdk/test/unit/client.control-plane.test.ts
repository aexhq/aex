/**
 * WS6 SDK control-plane clients: `client.orgs` / `client.workspaces` (plural,
 * management) / `client.keys`. These are INSTANCE FIELDS (like `client.workspace`
 * and `client.sessions`), so they never touch the prototype-reflection parity
 * gate. The one-time minted key (createWorkspace / keys.create) is wrapped in a
 * redacted `SecretString` — it never stringifies to its value.
 */
import { describe, expect, it, mock } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex, SecretString } from "../../src/index.js";

interface CapturedRequest {
  readonly url: string;
  readonly method: string;
  readonly body: unknown;
}

function makeStubFetch(routes: (req: CapturedRequest) => Response): { fetch: FetchLike; calls: CapturedRequest[] } {
  const calls: CapturedRequest[] = [];
  const stub: FetchLike = mock(async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = (init?.method ?? "GET").toString();
    let body: unknown = init?.body;
    if (typeof body === "string") {
      try {
        body = JSON.parse(body);
      } catch {
        /* leave as string */
      }
    }
    const req = { url, method, body };
    calls.push(req);
    return routes(req);
  });
  return { fetch: stub, calls };
}

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
}

function client(routes: (req: CapturedRequest) => Response) {
  const { fetch, calls } = makeStubFetch(routes);
  // A non-self-describing key keeps the explicit baseUrl (control-plane default).
  return { client: new Aex({ apiKey: "aexu_tkn", baseUrl: "https://x", fetch }), calls };
}

describe("Aex control-plane instance fields", () => {
  it("exposes orgs / workspaces / keys alongside the singular workspace context", () => {
    const c = new Aex({ apiKey: "aexu_tkn", baseUrl: "https://x" });
    expect(c.orgs).toBeDefined();
    expect(c.workspaces).toBeDefined();
    expect(c.keys).toBeDefined();
    // The singular data-plane context stays distinct from the plural collection.
    expect(c.workspace).toBeDefined();
    expect(c.workspace).not.toBe(c.workspaces);
  });
});

describe("client.orgs", () => {
  it("create posts the name and returns the org record", async () => {
    const { client: c, calls } = client(() => json({ org: { id: "org_1", name: "Acme", role: "admin" } }));
    const org = await c.orgs.create({ name: "Acme" });
    expect(org).toEqual({ id: "org_1", name: "Acme", role: "admin" });
    expect(calls[0]!.method).toBe("POST");
    expect(calls[0]!.url).toBe("https://x/api/orgs");
    expect(calls[0]!.body).toEqual({ name: "Acme" });
  });

  it("list / members / invite hit the expected routes", async () => {
    const { client: c, calls } = client((req) => {
      if (req.url.endsWith("/api/orgs") && req.method === "GET") return json({ orgs: [{ id: "org_1", name: "Acme" }] });
      if (req.url.endsWith("/api/orgs/org_1/members")) return json({ members: [{ appUserId: "u1", role: "admin" }] });
      if (req.url.endsWith("/api/orgs/org_1/invites")) return json({ invite: { id: "inv_1", orgId: "org_1", email: "a@b.test", role: "member" } });
      return json({}, 404);
    });
    expect((await c.orgs.list()).map((o) => o.id)).toEqual(["org_1"]);
    expect((await c.orgs.members("org_1")).map((m) => m.appUserId)).toEqual(["u1"]);
    const invite = await c.orgs.invite("org_1", { email: "a@b.test", role: "member" });
    expect(invite.id).toBe("inv_1");
    expect(calls.at(-1)!.body).toEqual({ email: "a@b.test", role: "member" });
  });
});

describe("client.workspaces (plural, control-plane management)", () => {
  it("create wraps the one-time key in a redacted SecretString", async () => {
    const { client: c, calls } = client(() =>
      json({ workspace: { workspaceId: "wsp_1", apiKey: "aex_dev_euw1_1_secret_crc", orgId: "org_1" } })
    );
    const created = await c.workspaces.create({ orgId: "org_1", name: "prod" });
    expect(created.workspaceId).toBe("wsp_1");
    expect(created.orgId).toBe("org_1");
    expect(created.apiKey).toBeInstanceOf(SecretString);
    // Redacted on coercion; only .unwrap() reveals it.
    expect(`${created.apiKey}`).toBe("[REDACTED]");
    expect(JSON.stringify({ k: created.apiKey })).toContain("[REDACTED]");
    expect(JSON.stringify({ k: created.apiKey })).not.toContain("aex_dev_euw1_1_secret_crc");
    expect(created.apiKey.unwrap()).toBe("aex_dev_euw1_1_secret_crc");
    expect(calls[0]!.method).toBe("POST");
    expect(calls[0]!.url).toBe("https://x/api/workspaces");
    expect(calls[0]!.body).toEqual({ orgId: "org_1", name: "prod" });
  });

  it("list / delete hit the control-plane routes", async () => {
    const { client: c, calls } = client((req) => {
      if (req.method === "DELETE") return new Response(null, { status: 204 });
      return json({ workspaces: [{ id: "wsp_1", name: "w", orgId: "org_1" }] });
    });
    expect((await c.workspaces.list()).map((w) => w.id)).toEqual(["wsp_1"]);
    await c.workspaces.delete("wsp_1");
    expect(calls.at(-1)!.method).toBe("DELETE");
    expect(calls.at(-1)!.url).toBe("https://x/api/workspaces/wsp_1");
  });
});

describe("client.keys", () => {
  it("create (workspace) wraps the minted key and posts workspaceId", async () => {
    const { client: c, calls } = client(() => json({ key: { id: "key_1", apiKey: "aex_dev_euw1_1_s_c", workspaceId: "wsp_1" } }));
    const minted = await c.keys.create({ workspaceId: "wsp_1", name: "ci" });
    expect(minted.id).toBe("key_1");
    expect(minted.apiKey).toBeInstanceOf(SecretString);
    expect(`${minted.apiKey}`).toBe("[REDACTED]");
    expect(minted.apiKey.unwrap()).toBe("aex_dev_euw1_1_s_c");
    expect(calls[0]!.body).toEqual({ workspaceId: "wsp_1", name: "ci" });
  });

  it("create (account PAT) posts account:true", async () => {
    const { client: c, calls } = client(() => json({ key: { id: "key_pat", apiKey: "aexu_pat", kind: "account" } }));
    const minted = await c.keys.create({ account: true });
    expect(minted.kind).toBe("account");
    expect(minted.apiKey.unwrap()).toBe("aexu_pat");
    expect(calls[0]!.body).toEqual({ account: true });
  });

  it("list returns metadata (no value) and delete targets the id", async () => {
    const { client: c, calls } = client((req) => {
      if (req.method === "DELETE") return new Response(null, { status: 204 });
      return json({ keys: [{ id: "key_1", kind: "workspace" }] });
    });
    const keys = await c.keys.list();
    expect(keys[0]!.id).toBe("key_1");
    expect("apiKey" in keys[0]!).toBe(false);
    await c.keys.delete("key_1");
    expect(calls.at(-1)!.method).toBe("DELETE");
    expect(calls.at(-1)!.url).toBe("https://x/api/keys/key_1");
  });
});
