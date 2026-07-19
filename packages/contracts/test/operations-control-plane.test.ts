/**
 * WS6 control-plane operations (orgs / workspaces / API keys). These target the
 * account/control-plane surface on the BFF; each request/response shape is
 * pinned here so the platform endpoints can be built to match, and the
 * one-time-reveal creates fail closed when the minted key is missing.
 */
import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/http.js";
import { operations } from "../src/internal.js";

const BASE = "https://api.test";

interface Captured {
  url?: string;
  method?: string;
  body?: unknown;
}

function clientFor(body: unknown, capture?: Captured, status = 200) {
  const fetchImpl = async (input: string | URL | Request, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    if (capture) {
      capture.url = url;
      capture.method = (init?.method ?? "GET").toUpperCase();
      capture.body = typeof init?.body === "string" ? (JSON.parse(init.body) as unknown) : undefined;
    }
    return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  };
  return new HttpClient({ apiKey: "aexu_tok", baseUrl: BASE, fetch: fetchImpl });
}

describe("orgs operations", () => {
  it("createOrg posts the name and unwraps { org }", async () => {
    const cap: Captured = {};
    const client = clientFor({ org: { id: "org_1", name: "Acme", role: "admin" } }, cap);
    const org = await operations.createOrg(client, { name: "Acme" });
    expect(cap.method).toBe("POST");
    expect(cap.url).toBe(`${BASE}/api/orgs`);
    expect(cap.body).toEqual({ name: "Acme" });
    expect(org).toEqual({ id: "org_1", name: "Acme", role: "admin" });
  });

  it("listOrgs unwraps the { orgs } array", async () => {
    const client = clientFor({ orgs: [{ id: "org_1", name: "Acme" }] });
    expect(await operations.listOrgs(client)).toEqual([{ id: "org_1", name: "Acme" }]);
  });

  it("listOrgs fails closed when the envelope is not an array", async () => {
    const client = clientFor({ orgs: {} });
    await expect(operations.listOrgs(client)).rejects.toThrow(/must contain a orgs array/);
  });

  it("listOrgMembers targets the org path and validates the id", async () => {
    const cap: Captured = {};
    const client = clientFor({ members: [{ appUserId: "u1", role: "admin" }] }, cap);
    await operations.listOrgMembers(client, "org_1");
    expect(cap.url).toBe(`${BASE}/api/orgs/org_1/members`);
    await expect(operations.listOrgMembers(client, "")).rejects.toThrow(/orgId must be a non-empty string/);
  });

  it("createOrgInvite posts to the org invites path", async () => {
    const cap: Captured = {};
    const client = clientFor({ invite: { id: "inv_1", orgId: "org_1", email: "a@b.test", role: "member" } }, cap);
    const invite = await operations.createOrgInvite(client, "org_1", { email: "a@b.test", role: "member" });
    expect(cap.method).toBe("POST");
    expect(cap.url).toBe(`${BASE}/api/orgs/org_1/invites`);
    expect(cap.body).toEqual({ email: "a@b.test", role: "member" });
    expect(invite.id).toBe("inv_1");
  });
});

describe("workspaces operations", () => {
  it("createWorkspace returns the one-time NewWorkspace", async () => {
    const cap: Captured = {};
    const client = clientFor(
      { workspace: { workspaceId: "wsp_1", apiKey: "aex_dev_euw1_1_s_c", orgId: "org_1" } },
      cap
    );
    const created = await operations.createWorkspace(client, { orgId: "org_1", name: "prod" });
    expect(cap.method).toBe("POST");
    expect(cap.url).toBe(`${BASE}/api/workspaces`);
    expect(cap.body).toEqual({ orgId: "org_1", name: "prod" });
    expect(created).toMatchObject({ workspaceId: "wsp_1", apiKey: "aex_dev_euw1_1_s_c" });
  });

  it("createWorkspace fails closed when the one-time key is missing", async () => {
    const client = clientFor({ workspace: { workspaceId: "wsp_1" } });
    await expect(operations.createWorkspace(client, { orgId: "org_1", name: "prod" })).rejects.toThrow(
      /missing the one-time apiKey/
    );
  });

  it("listWorkspaces unwraps { workspaces }", async () => {
    const client = clientFor({ workspaces: [{ id: "wsp_1", name: "w", orgId: "org_1" }] });
    expect(await operations.listWorkspaces(client)).toEqual([{ id: "wsp_1", name: "w", orgId: "org_1" }]);
  });

  it("deleteWorkspace issues a DELETE to the id path", async () => {
    const cap: Captured = {};
    const client = clientFor({}, cap);
    await operations.deleteWorkspace(client, "wsp_1");
    expect(cap.method).toBe("DELETE");
    expect(cap.url).toBe(`${BASE}/api/workspaces/wsp_1`);
  });
});

describe("api-key operations", () => {
  it("createApiKey mints a workspace key and returns the one-time value", async () => {
    const cap: Captured = {};
    const client = clientFor({ key: { id: "key_1", apiKey: "aex_dev_euw1_1_s_c", workspaceId: "wsp_1" } }, cap);
    const key = await operations.createApiKey(client, { workspaceId: "wsp_1", name: "ci" });
    expect(cap.method).toBe("POST");
    expect(cap.url).toBe(`${BASE}/api/keys`);
    expect(cap.body).toEqual({ workspaceId: "wsp_1", name: "ci" });
    expect(key).toMatchObject({ id: "key_1", apiKey: "aex_dev_euw1_1_s_c" });
  });

  it("createApiKey mints an account PAT with account:true", async () => {
    const cap: Captured = {};
    const client = clientFor({ key: { id: "key_pat", apiKey: "aexu_pat", kind: "account" } }, cap);
    await operations.createApiKey(client, { account: true });
    expect(cap.body).toEqual({ account: true });
  });

  it("createApiKey rejects passing both workspaceId and account (before transport)", async () => {
    const client = clientFor({});
    await expect(operations.createApiKey(client, { account: true, workspaceId: "wsp_1" })).rejects.toThrow(
      /not both/
    );
  });

  it("createApiKey fails closed when the one-time value is missing", async () => {
    const client = clientFor({ key: { id: "key_1" } });
    await expect(operations.createApiKey(client, { workspaceId: "wsp_1" })).rejects.toThrow(/missing the one-time apiKey/);
  });

  it("listApiKeys unwraps { keys } and deleteApiKey targets the id path", async () => {
    const client = clientFor({ keys: [{ id: "key_1", kind: "workspace" }] });
    expect(await operations.listApiKeys(client)).toEqual([{ id: "key_1", kind: "workspace" }]);

    const cap: Captured = {};
    const delClient = clientFor({}, cap);
    await operations.deleteApiKey(delClient, "key_1");
    expect(cap.method).toBe("DELETE");
    expect(cap.url).toBe(`${BASE}/api/keys/key_1`);
  });
});

describe("accountWhoami (control-plane PAT validation)", () => {
  it("GETs /api/whoami and parses an account_token principal", async () => {
    const cap: Captured = {};
    const client = clientFor(
      {
        ok: true,
        principalType: "account_token",
        appUserId: "usr_1",
        orgId: "org_1",
        tokenId: "atk_1",
        tokenName: "cli",
        tokenKind: "account",
        scopes: ["orgs:read"]
      },
      cap
    );
    const me = await operations.accountWhoami(client);
    expect(cap.method).toBe("GET");
    expect(cap.url).toBe(`${BASE}/api/whoami`);
    expect(me).toEqual({
      ok: true,
      principalType: "account_token",
      appUserId: "usr_1",
      orgId: "org_1",
      tokenId: "atk_1",
      tokenName: "cli",
      tokenKind: "account",
      scopes: ["orgs:read"]
    });
  });

  it("rejects an api_key principal (a workspace key is not an account PAT)", async () => {
    const client = clientFor({ ok: true, principalType: "api_key", workspaceId: "wsp_1", scopes: [] });
    await expect(operations.accountWhoami(client)).rejects.toThrow(/account_token principal/);
  });

  it("fails closed when appUserId is missing", async () => {
    const client = clientFor({ ok: true, principalType: "account_token", scopes: [] });
    await expect(operations.accountWhoami(client)).rejects.toThrow(/appUserId/);
  });
});
