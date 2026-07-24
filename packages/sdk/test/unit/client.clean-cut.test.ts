import { describe, expect, it } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex, type SessionFilesSnapshot } from "../../src/index.js";
import { SessionRunStream } from "../../src/client.js";
import type { Session } from "@aexhq/contracts";

const hash = `sha256:${"a".repeat(64)}`;
const EMPTY_SHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const file = {
  kind: "file" as const,
  resourceId: `wres_${"1".repeat(32)}`,
  version: 2,
  assetId: `asset_${"a".repeat(64)}`,
  contentHash: hash,
  name: "input",
  mountPath: "/workspace",
  sizeBytes: 12,
  contentType: "application/zip",
  createdAt: "2026-07-10T00:00:00.000Z"
};

function harness() {
  const calls: Array<{ url: string; method: string; body?: unknown }> = [];
  const fetch: FetchLike = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    calls.push({ url, method: init?.method ?? "GET", ...(body === undefined ? {} : { body }) });
    const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), {
      status,
      headers: { "content-type": "application/json" }
    });
    if (url.includes("/api/workspace/files")) return json({ resources: [file], nextCursor: "next-page" });
    if (new URL(url).pathname === "/api/sessions/session_1/files") return json({
      revision: {
        checkpointId: "cp_2",
        runId: "run_2",
        turnSeq: 2,
        committedAt: "2026-07-10T00:00:00.000Z",
        throughSeq: 44
      },
      files: [{ id: "output_1", checkpointId: "cp_2", filename: "result.txt", sizeBytes: 0, sha256: EMPTY_SHA256 }]
    });
    if (url.includes("/api/sessions/session_1/files/output_1/link?checkpointId=cp_2")) {
      return json({ url: "https://objects.example.test/output_1", expiresInSeconds: 3600 });
    }
    if (url.endsWith("/api/sessions/session_1")) return json({ session: { id: "session_1", status: "idle", acceptsMessages: true } });
    if (url.endsWith("/api/sessions")) return json({ session: { id: "session_1", status: "idle", acceptsMessages: true } }, 201);
    return json({});
  };
  return { client: new Aex({ apiKey: "token", baseUrl: "https://api.example.test", fetch }), calls };
}

describe("public SDK clean cut", () => {
  it("does not expose sessionId as a public Session alias", () => {
    const hasAlias: "sessionId" extends keyof Session ? true : false = false;
    expect(hasAlias).toBe(false);
  });

  it("exposes one workspace namespace and no legacy root resource clients", async () => {
    const { client, calls } = harness();
    expect((client as unknown as Record<string, unknown>).files).toBeUndefined();
    expect((client as unknown as Record<string, unknown>).skills).toBeUndefined();
    expect((client as unknown as Record<string, unknown>).secrets).toBeUndefined();
    expect((client.workspace.secrets as unknown as Record<string, unknown>)._createWorkspaceSecret).toBeUndefined();
    expect(await client.workspace.files.list({ cursor: "page-1", limit: 20 })).toEqual({
      resources: [file],
      nextCursor: "next-page"
    });
    expect(calls.at(-1)?.url).toBe("https://api.example.test/api/workspace/files?cursor=page-1&limit=20");
  });

  it("validates workspace page size before transport", async () => {
    const { client, calls } = harness();
    for (const limit of [0, 101, 1.5]) {
      await expect(client.workspace.files.list({ limit })).rejects.toThrow(/integer from 1 through 100/);
    }
    expect(calls).toHaveLength(0);
    await client.workspace.files.list({ limit: 100 });
    expect(calls.at(-1)?.url).toBe("https://api.example.test/api/workspace/files?limit=100");
  });

  it("submits resource refs under assets and keeps builtin tools separate", async () => {
    const { client, calls } = harness();
    await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
      assets: { files: [file] },
      builtinTools: "none",
    });
    const request = calls.find((call) => call.url.endsWith("/api/sessions") && call.method === "POST");
    const submission = (request?.body as { submission: Record<string, unknown> }).submission;
    expect(submission.assets).toEqual({ files: [file], skills: [], tools: [], instructions: [] });
    expect(submission.builtinTools).toBe("none");
    expect(submission).not.toHaveProperty("files");
    expect(submission).not.toHaveProperty("skills");
  });

  it("uses stable session namespace properties and returns checkpoint snapshots", async () => {
    const { client, calls } = harness();
    const session = await client.sessions.open("session_1");
    expect(session.messages).toBe(session.messages);
    expect(session.events).toBe(session.events);
    expect(session.files).toBe(session.files);
    expect(typeof session.messages).toBe("object");
    const snapshot: SessionFilesSnapshot = await session.files.list();
    const snapshotIsArray: typeof snapshot extends readonly unknown[] ? true : false = false;
    expect(snapshotIsArray).toBe(false);
    expect(Array.isArray(snapshot)).toBe(false);
    expect(snapshot).toMatchObject({
      revision: { checkpointId: "cp_2", runId: "run_2", turnSeq: 2 },
      files: [{ id: "output_1", checkpointId: "cp_2", filename: "result.txt", sizeBytes: 0, sha256: EMPTY_SHA256 }]
    });
    expect(snapshot.revision.checkpointId).toBe("cp_2");
    expect(snapshot.files[0]?.checkpointId).toBe("cp_2");
    await session.files.link(snapshot.files[0]!);
    expect(calls.slice(-2).map((call) => call.url)).toEqual([
      "https://api.example.test/api/sessions/session_1/files?checkpointId=cp_2",
      "https://api.example.test/api/sessions/session_1/files/output_1/link?checkpointId=cp_2"
    ]);
  });

  it("offers finished() as the only run completion method", () => {
    expect(typeof SessionRunStream.prototype.finished).toBe("function");
    expect((SessionRunStream.prototype as unknown as { done?: unknown }).done).toBeUndefined();
  });

  it("rejects the removed sessionId response alias at runtime", async () => {
    const fetch: FetchLike = async () => new Response(JSON.stringify({
      session: { id: "session_1", sessionId: "session_1", status: "idle", acceptsMessages: true }
    }), { status: 201, headers: { "content-type": "application/json" } });
    const client = new Aex({ apiKey: "token", baseUrl: "https://api.example.test", fetch });
    await expect(client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
    })).rejects.toThrow(/removed sessionId field/);
  });

  it("rejects flat session responses instead of guessing a legacy envelope", async () => {
    const fetch: FetchLike = async () => new Response(JSON.stringify({
      id: "session_1",
      status: "idle",
      acceptsMessages: true
    }), { status: 200, headers: { "content-type": "application/json" } });
    const client = new Aex({ apiKey: "token", baseUrl: "https://api.example.test", fetch });

    await expect(client.sessions.open("session_1")).rejects.toThrow(/must contain a session object/);
    await expect(client.sessions.get("session_1")).rejects.toThrow(/must contain a session object/);
  });

  it("accepts runtimeSize only on the wire and exposes runtime to callers", async () => {
    const response = (runtimeField: Record<string, unknown>) => new Response(JSON.stringify({
      session: {
        id: "session_1",
        status: "idle",
        acceptsMessages: true,
        ...runtimeField
      }
    }), { status: 200, headers: { "content-type": "application/json" } });
    const canonical = new Aex({
      apiKey: "token",
      baseUrl: "https://api.example.test",
      fetch: async () => response({ runtimeSize: "0.25cpu-1gb" })
    });
    const session = await canonical.sessions.open("session_1");
    expect(session.record.runtime).toEqual({ size: "0.25cpu-1gb" });
    expect(session.record).not.toHaveProperty("runtimeSize");

    const legacy = new Aex({
      apiKey: "token",
      baseUrl: "https://api.example.test",
      fetch: async () => response({ runtime: "0.25cpu-1gb" })
    });
    await expect(legacy.sessions.open("session_1")).rejects.toThrow(/removed runtime field/);
  });

  it.each([
    ["shared-0.25x-1gb", "0.25cpu-1gb"],
    ["shared-0.5x-4gb", "0.5cpu-4gb"],
    ["shared-1x-6gb", "1cpu-6gb"],
    ["shared-2x-8gb", "2cpu-8gb"],
    ["shared-4x-12gb", "4cpu-12gb"]
  ] as const)("normalizes a legacy read-side runtime-size token (%s)", async (legacy, canonical) => {
    const client = new Aex({
      apiKey: "token",
      baseUrl: "https://api.example.test",
      fetch: async () => new Response(JSON.stringify({
        session: {
          id: "session_1",
          status: "idle",
          acceptsMessages: true,
          runtimeSize: legacy
        }
      }), { status: 200, headers: { "content-type": "application/json" } })
    });

    const session = await client.sessions.open("session_1");
    expect(session.record.runtime).toEqual({ size: canonical });
  });

  it("still rejects the retired legacy runtime-size tier on read", async () => {
    const client = new Aex({
      apiKey: "token",
      baseUrl: "https://api.example.test",
      fetch: async () => new Response(JSON.stringify({
        session: {
          id: "session_1",
          status: "idle",
          acceptsMessages: true,
          runtimeSize: "shared-0.06x-256mb"
        }
      }), { status: 200, headers: { "content-type": "application/json" } })
    });

    await expect(client.sessions.open("session_1")).rejects.toThrow(/invalid runtimeSize/);
  });

  it("rejects bare child and webhook arrays", async () => {
    const fetch: FetchLike = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
      const value = url.endsWith("/children") || url.endsWith("/webhook-deliveries")
        ? []
        : { session: { id: "session_1", status: "idle", acceptsMessages: true } };
      return new Response(JSON.stringify(value), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    };
    const client = new Aex({ apiKey: "token", baseUrl: "https://api.example.test", fetch });
    const session = await client.sessions.open("session_1");

    await expect(session.children()).rejects.toThrow(/must contain a children array/);
    await expect(session.webhooks.list()).rejects.toThrow(/must contain a deliveries array/);
  });
});
