/**
 * `aex.workspace.secrets` — the workspace secret MANAGEMENT client, mirroring
 * the other workspace resource namespaces.
 *
 * Lifecycle parity with assets: a `Secret.value(...)` is per-session and
 * gone at terminal; `aex.workspace.secrets.set` persists a named,
 * searchable workspace secret you can `get` (metadata), `rotate`, `list`, and
 * `delete`.
 *
 * Posture (confirmed): `get` returns METADATA only (no value). Secret values are
 * write-only through the public SDK after create/rotate.
 */
import { describe, expect, it, vi } from "vitest";
import { Aex, Secret } from "../../src/index.js";

interface CapturedRequest {
  readonly url: string;
  readonly method: string;
  readonly body: unknown;
}

function makeStubFetch(routes: (req: CapturedRequest) => Response): {
  fetch: typeof fetch;
  calls: CapturedRequest[];
} {
  const calls: CapturedRequest[] = [];
  const stub: typeof fetch = vi.fn(async (input, init) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
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

const REC = {
  id: "sec_serper",
  name: "serper",
  version: 1,
  state: "ready" as const,
  createdAt: "2026-06-16T00:00:00Z",
  updatedAt: "2026-06-16T00:00:00Z"
};

function client(routes: (req: CapturedRequest) => Response) {
  const { fetch, calls } = makeStubFetch(routes);
  return { client: new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch }), calls };
}

describe("aex.workspace.secrets management client", () => {
  it("set: creates a named workspace secret (value in body, not URL)", async () => {
    const { client: c, calls } = client(() => json({ secret: REC }));
    const rec = await c.workspace.secrets.set({ name: "serper", value: "sk-live-XYZ" });
    expect(rec.name).toBe("serper");
    expect(rec.version).toBe(1);
    const call = calls[0]!;
    expect(call.method).toBe("POST");
    expect(call.url).toBe("https://x/api/secrets");
    expect(call.body).toEqual({ name: "serper", value: "sk-live-XYZ" });
  });

  it("list: returns workspace secret metadata (searchable by name)", async () => {
    const { client: c, calls } = client(() => json({ secrets: [REC] }));
    const list = await c.workspace.secrets.list();
    expect(list.map((s) => s.name)).toEqual(["serper"]);
    expect(calls[0]!.method).toBe("GET");
    expect(calls[0]!.url).toBe("https://x/api/secrets");
  });

  it("rejects legacy flat secret responses", async () => {
    const flatRecord = client(() => json(REC));
    await expect(flatRecord.client.workspace.secrets.get("serper")).rejects.toThrow(/must contain a secret object/);

    const bareList = client(() => json([REC]));
    await expect(bareList.client.workspace.secrets.list()).rejects.toThrow(/must contain a secrets array/);
  });

  it("get: returns METADATA only — no value field on the wire", async () => {
    const { client: c, calls } = client(() => json({ secret: REC }));
    const rec = await c.workspace.secrets.get("serper");
    expect(rec.name).toBe("serper");
    expect("value" in rec).toBe(false);
    expect(calls[0]!.method).toBe("GET");
    expect(calls[0]!.url).toBe("https://x/api/secrets/serper");
  });

  it("rotate: replaces the value (value in body)", async () => {
    const { client: c, calls } = client(() => json({ secret: { ...REC, version: 2 } }));
    const rec = await c.workspace.secrets.rotate({ name: "serper", value: "sk-new-1" });
    expect(rec.version).toBe(2);
    expect(calls[0]!.method).toBe("POST");
    expect(calls[0]!.url).toBe("https://x/api/secrets/serper/rotate");
    expect(calls[0]!.body).toEqual({ value: "sk-new-1" });
  });

  it("delete: removes a workspace secret by name", async () => {
    const { client: c, calls } = client(() => new Response(null, { status: 204 }));
    await c.workspace.secrets.delete("serper");
    expect(calls[0]!.method).toBe("DELETE");
    expect(calls[0]!.url).toBe("https://x/api/secrets/serper");
  });

  it("never puts a secret value in a URL or query string", async () => {
    const { client: c, calls } = client(() => json({ secret: REC }));
    await c.workspace.secrets.set({ name: "serper", value: "sk-live-XYZ" });
    await c.workspace.secrets.rotate({ name: "serper", value: "sk-new-1" });
    await c.workspace.secrets.get("serper");
    for (const call of calls) {
      expect(call.url).not.toContain("sk-live-XYZ");
      expect(call.url).not.toContain("sk-new-1");
    }
  });
});

describe("workspace secret reference flow", () => {
  it("persists through the workspace namespace and references the returned name", async () => {
    const { client: c, calls } = client(() => json({ secret: REC }));
    const record = await c.workspace.secrets.set({ name: "serper", value: "sk-live-XYZ" });
    const ref = Secret.ref(record.name);
    expect(ref.kind).toBe("ref");
    expect(ref.handle).toBe("serper");
    expect(ref.toSubmissionEntry()).toEqual({ ref: "serper" });
    const post = calls.find((call) => call.method === "POST")!;
    expect(post.url).toBe("https://x/api/secrets");
    expect(post.body).toEqual({ name: "serper", value: "sk-live-XYZ" });
    expect(post.url).not.toContain("sk-live-XYZ");
  });
});
