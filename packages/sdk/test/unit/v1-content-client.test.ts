import { describe, expect, it } from "bun:test";
import { newId, type FetchLike } from "@aexhq/contracts";
import { Aex } from "../../src/index.js";

const BASE_URL = "https://eu-west-1.api.aex.test";
const SID = newId("session");
const WID = newId("workspace");
const RID = newId("run");
const AID = newId("agent");
const TCID = newId("toolCall");
const APID = newId("approval");
const GID = newId("generation");
const UID = newId("upload");
const MSRID = newId("measurement");
const at = "2026-07-30T10:00:00.000Z";
const hash = `sha256:${"a".repeat(64)}`;

type Call = {
  readonly url: URL;
  readonly method: string;
  readonly headers: Headers;
  readonly body: unknown;
};

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" }
  });
}

function session(): Record<string, unknown> {
  return {
    id: SID,
    workspaceId: WID,
    status: "idle",
    revision: 7,
    persistRevision: 2,
    createdAt: at,
    updatedAt: at,
    continuity: {
      state: "cold",
      persistedRevision: 2,
      changedAt: at,
      reason: "not_started"
    },
    lineage: {},
    resolvedConfig: {}
  };
}

const entry = {
  path: "reports/result.csv",
  type: "file",
  sizeBytes: 12,
  sha256: hash,
  mode: "0644",
  mtime: at
} as const;

const grant = {
  url: "https://download.example.test/object",
  headers: { Range: "bytes=0-11" },
  expiresAt: "2026-07-30T10:05:00.000Z",
  sizeBytes: 12,
  authorizedBytes: 12,
  measurementId: MSRID,
  sha256: hash
} as const;

const approval = {
  id: APID,
  sessionId: SID,
  runId: RID,
  agentId: AID,
  toolCallId: TCID,
  toolName: "github.create_issue",
  argumentsSha256: hash,
  implementationSha256: hash,
  expectedGenerationId: GID,
  status: "pending",
  requestedAt: at,
  updatedAt: at
} as const;

function clientWith(
  respond: (call: Call) => Response | Promise<Response>
): { readonly client: Aex; readonly calls: Call[] } {
  const calls: Call[] = [];
  const fetch: FetchLike = async (input, init) => {
    const call: Call = {
      url: new URL(input instanceof URL ? input.href : String(input)),
      method: init?.method ?? "GET",
      headers: new Headers(init?.headers),
      body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined
    };
    calls.push(call);
    return respond(call);
  };
  return {
    client: new Aex({ apiKey: "test-key", baseUrl: BASE_URL, fetch, retry: false }),
    calls
  };
}

describe("v1 session file and approval namespaces", () => {
  it("uses the exact persisted/live action routes and access controls", async () => {
    const { client, calls } = clientWith((call) => {
      if (call.url.pathname === `/api/sessions/${SID}`) return json(session());
      if (call.url.pathname.endsWith("/persisted/list")) return json({ items: [entry] });
      if (call.url.pathname.endsWith("/persisted/stat")) return json(entry);
      if (call.url.pathname.endsWith("/persisted/downloads")) return json(grant);
      const workspaceAccess = { generationId: GID, resumed: false };
      if (call.url.pathname.endsWith("/live/list")) {
        return json({ items: [entry], workspaceAccess });
      }
      if (call.url.pathname.endsWith("/live/stat")) {
        return json({ ...entry, workspaceAccess });
      }
      if (call.url.pathname.endsWith("/live/downloads")) {
        return json({ ...grant, workspaceAccess });
      }
      throw new Error(`no responder for ${call.method} ${call.url.pathname}`);
    });
    const opened = await client.sessions.open(SID);

    await opened.files.persisted.list({ path: "reports", recursive: true });
    await opened.files.persisted.stat({ path: entry.path });
    await opened.files.persisted.download({
      path: entry.path,
      range: { start: 0, endExclusive: 12 }
    });
    await opened.files.live.list({
      path: "reports",
      wake: "never",
      consistency: "best_effort",
      ifGenerationId: GID
    });
    await opened.files.live.stat({ path: entry.path, wake: "retained" });
    await opened.files.live.download({ path: entry.path, ifGenerationId: GID });

    expect(calls.slice(1).map(({ method, url }) => `${method} ${url.pathname}`)).toEqual([
      `POST /api/sessions/${SID}/files/persisted/list`,
      `POST /api/sessions/${SID}/files/persisted/stat`,
      `POST /api/sessions/${SID}/files/persisted/downloads`,
      `POST /api/sessions/${SID}/files/live/list`,
      `POST /api/sessions/${SID}/files/live/stat`,
      `POST /api/sessions/${SID}/files/live/downloads`
    ]);
    expect(calls[3]!.body).toEqual({
      path: entry.path,
      range: { start: 0, endExclusive: 12 }
    });
    expect(calls[4]!.body).toMatchObject({
      wake: "never",
      consistency: "best_effort",
      ifGenerationId: GID
    });
    expect(calls[1]!.headers.get("idempotency-key")).toBeNull();
    expect(calls[3]!.headers.get("idempotency-key")).toBeTruthy();
    expect(calls[6]!.headers.get("idempotency-key")).toBeTruthy();
  });

  it("lists, gets, and responds to one exact-call approval without blanket controls", async () => {
    const { client, calls } = clientWith((call) => {
      if (call.url.pathname === `/api/sessions/${SID}`) return json(session());
      if (call.url.pathname.endsWith("/approvals")) return json({ items: [approval] });
      if (call.method === "POST") {
        return json({ ...approval, status: "approved", decision: "approve", resolvedAt: at });
      }
      return json(approval);
    });
    const opened = await client.sessions.open(SID);
    await opened.approvals.list();
    await opened.approvals.get(APID);
    const winner = await opened.approvals.respond(APID, { decision: "approve" });

    expect(winner.status).toBe("approved");
    if (winner.status !== "approved") throw new Error("expected approved winner");
    expect(winner.decision).toBe("approve");
    expect(calls.at(-1)!.url.pathname).toBe(
      `/api/sessions/${SID}/approvals/${APID}/responses`
    );
    expect(calls.at(-1)!.body).toEqual({ decision: "approve" });
    for (const name of ["approve", "deny", "requestApproval"]) {
      expect((opened as unknown as Record<string, unknown>)[name]).toBeUndefined();
    }
  });
});

describe("v1 workspace registries, uploads, and secrets", () => {
  it("provides overwrite-only registries with explicit revision headers", async () => {
    const resource = {
      kind: "instruction",
      name: "review",
      revision: 3,
      state: "current",
      sha256: hash,
      sizeBytes: 12,
      value: { text: "Review carefully." },
      createdAt: at,
      updatedAt: at
    };
    const { client, calls } = clientWith((call) => {
      if (call.method === "DELETE") return new Response(null, { status: 204 });
      if (call.url.pathname === "/api/workspace/instructions") {
        const { value: _value, ...summary } = resource;
        return json({ items: [summary] });
      }
      return json(resource);
    });

    expect((await client.workspace.instructions.list()).items).toHaveLength(1);
    await client.workspace.instructions.get("review");
    await client.workspace.instructions.set(
      "review",
      { text: "Review carefully." },
      { ifRevision: 2 }
    );
    await client.workspace.instructions.delete("review");

    expect(calls.map(({ method, url }) => `${method} ${url.pathname}`)).toEqual([
      "GET /api/workspace/instructions",
      "GET /api/workspace/instructions/review",
      "PUT /api/workspace/instructions/review",
      "DELETE /api/workspace/instructions/review"
    ]);
    expect(calls[2]!.headers.get("idempotency-key")).toBeTruthy();
    expect(calls[2]!.headers.get("if-match")).toBe("\"2\"");
    expect(calls[3]!.headers.get("if-match")).toBeNull();
    for (const registry of [
      client.workspace.files,
      client.workspace.skills,
      client.workspace.tools,
      client.workspace.instructions,
      client.workspace.mcpServers
    ]) {
      for (const removed of ["publish", "copy", "versions", "history", "archive", "restore"]) {
        expect((registry as unknown as Record<string, unknown>)[removed]).toBeUndefined();
      }
    }
  });

  it("stages uploads through create/parts/completion/abort only", async () => {
    const upload = {
      id: UID,
      state: "pending",
      sizeBytes: 12,
      sha256: hash,
      contentType: "application/gzip",
      createdAt: at,
      expiresAt: "2026-07-31T10:00:00.000Z"
    };
    const { client, calls } = clientWith((call) => {
      if (call.method === "DELETE") return new Response(null, { status: 204 });
      if (call.url.pathname.endsWith("/parts")) {
        return json({
          parts: [{
            partNumber: 1,
            url: "https://upload.example.test/part",
            headers: {},
            expiresAt: "2026-07-30T10:05:00.000Z"
          }]
        });
      }
      return json(call.url.pathname.endsWith("/completion")
        ? { ...upload, state: "ready" }
        : upload, call.url.pathname === "/api/workspace/uploads" ? 201 : 200);
    });

    await client.workspace.uploads.create({
      sizeBytes: 12,
      sha256: hash,
      contentType: "application/gzip"
    });
    await client.workspace.uploads.parts(UID, { partNumbers: [1] });
    await client.workspace.uploads.complete(UID, {
      parts: [{ partNumber: 1, etag: "\"etag\"" }]
    });
    await client.workspace.uploads.abort(UID);

    expect(calls.map(({ method, url }) => `${method} ${url.pathname}`)).toEqual([
      "POST /api/workspace/uploads",
      `POST /api/workspace/uploads/${UID}/parts`,
      `POST /api/workspace/uploads/${UID}/completion`,
      `DELETE /api/workspace/uploads/${UID}`
    ]);
    expect(calls[0]!.headers.get("idempotency-key")).toBeTruthy();
    expect(calls[1]!.headers.get("idempotency-key")).toBeNull();
    expect(calls[2]!.headers.get("idempotency-key")).toBeTruthy();
  });

  it("keeps secret reads metadata-only and exposes explicit revocation", async () => {
    const metadata = {
      name: "GITHUB_TOKEN",
      revision: 2,
      state: "ready",
      createdAt: at,
      updatedAt: at
    };
    const { client, calls } = clientWith((call) => {
      if (call.method === "DELETE") return new Response(null, { status: 204 });
      if (call.url.pathname.endsWith("/revocations")) {
        return json({ name: metadata.name, revision: 3, revokedAt: at });
      }
      if (call.url.pathname === "/api/workspace/secrets") {
        return json({ items: [metadata] });
      }
      return json(metadata);
    });

    await client.workspace.secrets.list();
    await client.workspace.secrets.get(metadata.name);
    await client.workspace.secrets.set(metadata.name, "plaintext", { ifRevision: 1 });
    await client.workspace.secrets.delete(metadata.name);
    await client.workspace.secrets.revoke(metadata.name);

    expect(calls[2]!.body).toEqual({ value: "plaintext" });
    expect(calls[2]!.headers.get("if-match")).toBe("\"1\"");
    expect(calls[4]!.url.pathname).toBe(
      `/api/workspace/secrets/${metadata.name}/revocations`
    );
    for (const call of [calls[0]!, calls[1]!, calls[4]!]) {
      expect(JSON.stringify(call.body ?? {})).not.toContain("plaintext");
    }
  });

  it("removes legacy asset/version/checkpoint and standalone secret builders", async () => {
    const root = await import("../../src/index.js") as Record<string, unknown>;
    for (const name of [
      "AssetIdentity",
      "WorkspaceFileRef",
      "McpServer",
      "Secret",
      "File",
      "Skill",
      "Tool",
      "Instructions"
    ]) {
      expect(root[name]).toBeUndefined();
    }
  });
});
