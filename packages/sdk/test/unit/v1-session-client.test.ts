import { describe, expect, it } from "bun:test";
import { idPattern, newId, type FetchLike } from "@aexhq/contracts";
import {
  Aex,
  OperationFailedError,
  RunFailedError
} from "../../src/index.js";

const BASE_URL = "https://eu-west-1.api.aex.test";
const SID = newId("session");
const WID = newId("workspace");
const MID = newId("message");
const OUTPUT_MID = newId("message");
const RID = newId("run");
const OID = newId("operation");
const GID = newId("generation");

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

function session(id = SID): Record<string, unknown> {
  return {
    id,
    workspaceId: WID,
    status: "idle",
    revision: 7,
    persistRevision: 2,
    createdAt: "2026-07-30T10:00:00.000Z",
    updatedAt: "2026-07-30T10:00:00.000Z",
    continuity: {
      state: "cold",
      persistedRevision: 2,
      changedAt: "2026-07-30T10:00:00.000Z",
      reason: "not_started"
    },
    lineage: {},
    resolvedConfig: {}
  };
}

function run(status: string, overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: RID,
    sessionId: SID,
    messageId: MID,
    status,
    maxSpendCents: 100,
    queuedAt: "2026-07-30T10:00:00.000Z",
    ...overrides
  };
}

function operation(
  kind: string,
  status: string,
  id: string = OID,
  overrides: Record<string, unknown> = {}
): Record<string, unknown> {
  return {
    id,
    workspaceId: WID,
    sessionId: SID,
    kind,
    status,
    cancelable: status === "queued",
    createdAt: "2026-07-30T10:00:00.000Z",
    updatedAt: "2026-07-30T10:00:00.000Z",
    ...overrides
  };
}

function recordingClient(
  respond: (call: Call, index: number) => Response | Promise<Response>
): { readonly client: Aex; readonly calls: Call[] } {
  const calls: Call[] = [];
  const fetch: FetchLike = async (input, init) => {
    const headers = new Headers(init?.headers);
    const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    const call: Call = {
      url: new URL(input instanceof URL ? input.href : String(input)),
      method: init?.method ?? "GET",
      headers,
      body
    };
    calls.push(call);
    return respond(call, calls.length - 1);
  };
  return {
    client: new Aex({ apiKey: "test-key", baseUrl: BASE_URL, fetch, retry: false }),
    calls
  };
}

describe("v1 session resources", () => {
  it("derives the regional endpoint from the canonical workspace-key parser", async () => {
    const secret = Buffer.alloc(32, 0xa5).toString("base64url");
    const keyId = newId("apiKey");
    const apiKey = `aex_wk_use2_${keyId.slice("key_".length)}_${secret}`;
    let origin: string | undefined;
    const client = new Aex({
      apiKey,
      fetch: async (input) => {
        origin = new URL(input instanceof URL ? input.href : String(input)).origin;
        return json({ items: [] });
      },
      retry: false
    });

    await client.sessions.list();
    expect(origin).toBe("https://us-east-2.api.aex.dev");
  });

  it("exposes create/get/open/list and makes open exactly one observational GET", async () => {
    const { client, calls } = recordingClient((call) => {
      if (call.method === "POST") return json(session(), 201);
      if (call.url.searchParams.has("status")) {
        return json({ items: [session()], nextCursor: "cur_next" });
      }
      return json(session());
    });

    const created = await client.sessions.create({ model: "openai/gpt-5" });
    const got = await client.sessions.get(SID);
    const opened = await client.sessions.open(SID);
    const listed = await client.sessions.list({ status: "idle", limit: 2 });

    expect(created.id).toBe(SID);
    expect(got.id).toBe(SID);
    expect(opened.id).toBe(SID);
    expect(listed.items).toHaveLength(1);
    expect(calls.map((call) => `${call.method} ${call.url.pathname}`)).toEqual([
      "POST /api/sessions",
      `GET /api/sessions/${SID}`,
      `GET /api/sessions/${SID}`,
      "GET /api/sessions"
    ]);
    expect(calls[0]!.headers.get("idempotency-key")).toBeTruthy();
    expect(calls[0]!.headers.get("aex-operation-id")).toBeNull();
  });

  it("normalizes messages.send(\"text\") and returns a message plus a polling run handle", async () => {
    let runReads = 0;
    const { client, calls } = recordingClient((call) => {
      if (call.url.pathname === `/api/sessions/${SID}`) return json(session());
      if (call.method === "POST") {
        return json({
          message: {
            id: MID,
            sessionId: SID,
            runId: RID,
            role: "user",
            content: [{ type: "text", text: "hello" }],
            createdAt: "2026-07-30T10:00:00.000Z"
          },
          run: run("queued")
        }, 201);
      }
      runReads += 1;
      return json(run("succeeded", {
        terminalAt: "2026-07-30T10:00:01.000Z",
        outputMessageIds: [OUTPUT_MID]
      }));
    });

    const opened = await client.sessions.open(SID);
    const accepted = await opened.messages.send("hello");
    const finished = await accepted.run.result({ pollIntervalMs: 0 });

    expect(accepted.message.id).toBe(MID);
    expect(accepted.run.id).toBe(RID);
    expect(finished.status).toBe("succeeded");
    expect(runReads).toBe(1);
    expect(calls[1]!.body).toEqual({
      content: [{ type: "text", text: "hello" }]
    });
    expect(calls[1]!.headers.get("idempotency-key")).toBeTruthy();
    expect(calls.map((call) => call.url.pathname)).not.toContain(
      `/api/sessions/${SID}/runs/${RID}/result`
    );
  });

  it("throws a typed terminal run failure from result()", async () => {
    const { client } = recordingClient((call) => {
      if (call.url.pathname === `/api/sessions/${SID}`) return json(session());
      if (call.method === "POST") {
        return json({ message: {}, run: run("failed", {
          error: {
            code: "run_failed",
            message: "boom",
            requestId: "req_test",
            retryable: false
          }
        }) }, 201);
      }
      throw new Error("unexpected request");
    });

    const opened = await client.sessions.open(SID);
    const accepted = await opened.messages.send({ content: [{ type: "text", text: "hello" }] });
    await expect(accepted.run.result()).rejects.toBeInstanceOf(RunFailedError);
  });

  it("rejects invalid address IDs before HTTP and rejects invalid returned run IDs", async () => {
    const untouched = recordingClient(() => {
      throw new Error("HTTP must not run");
    });
    await expect(untouched.client.sessions.get("session_bad")).rejects.toThrow(
      /sessionId must match/
    );
    await expect(untouched.client.sessions.open("session_bad")).rejects.toThrow(
      /sessionId must match/
    );
    await expect(untouched.client.operations.get("operation_bad")).rejects.toThrow(
      /operationId must match/
    );
    await expect(untouched.client.operations.open("operation_bad")).rejects.toThrow(
      /operationId must match/
    );
    await expect(untouched.client.operations.cancel("operation_bad")).rejects.toThrow(
      /operationId must match/
    );
    expect(untouched.calls).toEqual([]);

    const invalidRun = recordingClient((call) => {
      if (call.method === "GET") return json(session());
      return json({ message: {}, run: { ...run("queued"), id: "run_bad" } }, 201);
    });
    const opened = await invalidRun.client.sessions.open(SID);
    await expect(opened.messages.send("hello")).rejects.toThrow(
      /run.id must match/
    );
  });
});

describe("v1 durable operation handles", () => {
  it("sends exactly one SDK-generated operation ID and only explicit revisions", async () => {
    const { client, calls } = recordingClient((call) => {
      if (call.method === "GET") return json(session());
      const id = call.headers.get("aex-operation-id")!;
      const kindByPath: Record<string, string> = {
        stops: "session_stop",
        persists: "session_persist",
        forks: "session_fork",
        discards: "workspace_discard",
        "credential-rebinds": "credential_rebind",
        deletions: "session_delete"
      };
      const leaf = call.url.pathname.split("/").at(-1)!;
      return json(operation(kindByPath[leaf]!, "queued", id), 202);
    });

    const opened = await client.sessions.open(SID);
    const handles = [
      await opened.stop(),
      await opened.persist({ include: ["reports/**"] }, { ifRevision: 7 }),
      await opened.fork({ files: "current", credentials: "copy" }),
      await opened.workspace.discard({ ifGenerationId: GID }),
      await opened.credentials.rebind({ secrets: [{ name: "GITHUB_TOKEN" }] }),
      await opened.delete({ cascade: false })
    ];

    expect(handles.map((handle) => handle.id)).toHaveLength(6);
    for (const call of calls.slice(1)) {
      expect(call.headers.get("aex-operation-id")).toMatch(idPattern("operation"));
      expect(call.headers.get("idempotency-key")).toBeNull();
      expect(call.body).not.toHaveProperty("operationId");
    }
    expect(new Set(handles.map((handle) => handle.id)).size).toBe(6);
    expect(calls[1]!.headers.get("if-match")).toBeNull();
    expect(calls[2]!.headers.get("if-match")).toBe("\"7\"");
    expect(calls[3]!.headers.get("if-match")).toBeNull();
  });

  it("reuses that operation ID across automatic transport retries", async () => {
    const ids: string[] = [];
    let attempts = 0;
    const fetch: FetchLike = async (input, init) => {
      const url = new URL(input instanceof URL ? input.href : String(input));
      if (url.pathname === `/api/sessions/${SID}`) return json(session());
      const headers = new Headers(init?.headers);
      ids.push(headers.get("aex-operation-id")!);
      attempts += 1;
      if (attempts === 1) {
        return json({
          error: {
            code: "temporarily_unavailable",
            message: "retry",
            requestId: "req_retry",
            retryable: true
          }
        }, 503);
      }
      return json(operation("session_stop", "queued", ids[0]), 202);
    };
    const client = new Aex({
      apiKey: "test-key",
      baseUrl: BASE_URL,
      fetch,
      retry: {
        maxAttempts: 2,
        initialDelayMs: 0,
        maxDelayMs: 0,
        maxElapsedMs: 100
      }
    });

    const opened = await client.sessions.open(SID);
    await opened.stop();

    expect(ids).toHaveLength(2);
    expect(ids[0]).toMatch(idPattern("operation"));
    expect(ids[1]).toBe(ids[0]);
  });

  it("exposes operations get/open/list/cancel and polls only the operation GET route", async () => {
    let operationReads = 0;
    const success = {
      sessionId: SID,
      changed: true,
      sessionRevision: 8
    };
    const { client, calls } = recordingClient((call) => {
      if (call.method === "POST") return json(operation("session_stop", "cancelled"));
      if (call.url.pathname === "/api/operations") {
        return json({ items: [operation("session_stop", "queued")] });
      }
      operationReads += 1;
      if (operationReads < 3) return json(operation("session_stop", "queued"));
      return json(operation("session_stop", "succeeded", OID, {
        result: success,
        terminalAt: "2026-07-30T10:00:02.000Z"
      }));
    });

    expect((await client.operations.get(OID)).status).toBe("queued");
    const opened = await client.operations.open(OID);
    expect(operationReads).toBe(2);
    expect((await opened.wait({ pollIntervalMs: 0 })).status).toBe("succeeded");
    expect(await opened.result({ pollIntervalMs: 0 })).toEqual(success);
    expect((await client.operations.list({ kind: "session_stop" })).items).toHaveLength(1);
    expect((await client.operations.cancel(OID)).status).toBe("cancelled");

    expect(calls.some((call) => /\/(wait|result)$/.test(call.url.pathname))).toBe(false);
  });

  it("rejects an operation whose kind changes on a later GET", async () => {
    let reads = 0;
    const { client } = recordingClient(() => {
      reads += 1;
      return json(operation(
        reads === 1 ? "session_stop" : "session_persist",
        "queued"
      ));
    });
    const opened = await client.operations.open(OID);
    await expect(opened.wait({ pollIntervalMs: 0 })).rejects.toThrow(
      /changed kind/
    );
  });

  it("returns typed success values from initiating handles and typed terminal failures", async () => {
    let fail = false;
    const { client } = recordingClient((call) => {
      if (call.url.pathname === `/api/sessions/${SID}`) return json(session());
      if (call.method === "POST") {
        return json(operation(
          "session_stop",
          fail ? "failed" : "succeeded",
          call.headers.get("aex-operation-id")!,
          fail
          ? {
              error: {
                code: "operation_failed",
                message: "boom",
                requestId: "req_test",
                retryable: false
              }
            }
          : {
              result: { sessionId: SID, changed: false, sessionRevision: 7 }
            }
        ), 202);
      }
      throw new Error("unexpected request");
    });
    const opened = await client.sessions.open(SID);
    const stopped = await opened.stop();
    const result: { readonly changed: boolean } = await stopped.result();
    expect(result.changed).toBe(false);

    fail = true;
    const failed = await opened.stop();
    await expect(failed.result()).rejects.toBeInstanceOf(OperationFailedError);
  });
});

describe("removed v1 SDK surface", () => {
  it("does not expose legacy start/runtime/checkpoint/child/webhook APIs", async () => {
    const root = await import("../../src/index.js") as Record<string, unknown>;
    expect((Aex.prototype as unknown as Record<string, unknown>).start).toBeUndefined();
    for (const name of [
      "RuntimeKinds",
      "Sizes",
      "ChildSessionHandle",
      "SessionRunStream",
      "verifyAexWebhook"
    ]) {
      expect(root[name]).toBeUndefined();
    }

    const { client } = recordingClient(() => json(session()));
    const opened = await client.sessions.open(SID);
    for (const name of [
      "suspend",
      "resume",
      "restore",
      "capture",
      "checkpoint",
      "children",
      "webhooks"
    ]) {
      expect((opened as unknown as Record<string, unknown>)[name]).toBeUndefined();
    }
  });
});
