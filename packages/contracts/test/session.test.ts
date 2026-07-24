import { describe, expect, it } from "bun:test";
import {
  SESSION_STATUSES,
  isTerminalSessionStatus,
  type HttpClient,
  type SessionRetentionPolicy,
  type SessionStatus
} from "../src/index.js";
import { operations } from "../src/internal.js";

function httpStub(): { readonly http: HttpClient; readonly calls: Array<{ readonly path: string; readonly init: RequestInit; readonly query: Record<string, string> }> } {
  const calls: Array<{ readonly path: string; readonly init: RequestInit; readonly query: Record<string, string> }> = [];
  const http = {
    async request<T>(path: string, init: RequestInit = {}, query: Record<string, string> = {}): Promise<T> {
      calls.push({ path, init, query });
      if (path === "/api/sessions") {
        return { session: { id: "sess_1", status: "idle", acceptsMessages: true } } as T;
      }
      if (path.endsWith("/messages")) {
        if (init.method !== "POST") {
          return {
            messages: [
              {
                id: "msg_1",
                sender: "assistant",
                text: "hello",
                timestamp: "2026-07-02T12:00:00.000Z",
                turnSeq: 1,
                sequence: 1,
                messageId: "provider-msg-1"
              }
            ],
            nextCursor: "cursor-2"
          } as T;
        }
        return {
          session: { id: "sess_1", status: "running", acceptsMessages: false },
          run: { sessionId: "sess_1", runId: "run_1", turnSeq: 1, phase: "running" },
          eventCursor: 10
        } as T;
      }
      if (path.endsWith("/events/ticket")) {
        return { wsUrl: "wss://events.example.test/sess_1", ticket: "ticket", expiresAtMs: 1 } as T;
      }
      return { session: { id: "sess_1", status: "idle", acceptsMessages: true } } as T;
    }
  } as HttpClient;
  return { http, calls };
}

describe("session contracts", () => {
  it("keeps create separate from platform-only and first-message fields", () => {
    const noInput: "input" extends keyof import("../src/index.js").SessionCreateRequest ? true : false = false;
    const noWorkspace: "workspaceId" extends keyof import("../src/index.js").SessionCreateRequest ? true : false = false;
    const noMachine: "machine" extends keyof import("../src/index.js").SessionCreateRequest ? true : false = false;
    expect({ noInput, noWorkspace, noMachine }).toEqual({ noInput: false, noWorkspace: false, noMachine: false });
  });

  it("defines the resumable session status vocabulary separately from terminal sessions", () => {
    const statuses = new Set<SessionStatus>(SESSION_STATUSES);
    expect(statuses.has("idle")).toBe(true);
    expect(statuses.has("suspended")).toBe(true);
    expect(statuses.has("cancelling")).toBe(true);
    expect(statuses.has("deleted")).toBe(true);
    expect(isTerminalSessionStatus("idle")).toBe(false);
    expect(isTerminalSessionStatus("suspended")).toBe(false);
  });

  it("posts session messages with an Idempotency-Key header", async () => {
    const { http, calls } = httpStub();
    await operations.sendSessionMessage(
      http,
      "sess_1",
      { input: "continue" },
      { idempotencyKey: "idem-message" }
    );

    expect(calls).toHaveLength(1);
    expect(calls[0]!.path).toBe("/api/sessions/sess_1/messages");
    expect(calls[0]!.init.method).toBe("POST");
    expect(calls[0]!.init.headers).toEqual({ "Idempotency-Key": "idem-message" });
    expect(JSON.parse(calls[0]!.init.body as string)).toEqual({ input: "continue" });
  });

  it("createSessionWithMessage creates the session and dispatches the first run with a derived message key", async () => {
    const { http, calls } = httpStub();
    const result = await operations.createSessionWithMessage(
      http,
      {
        submission: {
          model: "deepseek/deepseek-v4-flash",
          assets: { files: [], skills: [], tools: [], instructions: [] },
          builtinTools: "default",
          mcpServers: []
        },
        secrets: {}
      },
      "start now",
      { idempotencyKey: "idem-create" }
    );

    expect(result.session.id).toBe("sess_1");
    expect(result.run.sessionId).toBe("sess_1");
    expect(result.session.status).toBe("running");
    expect(calls).toHaveLength(2);
    expect(calls[0]!.path).toBe("/api/sessions");
    expect(calls[0]!.init.method).toBe("POST");
    expect(calls[0]!.init.headers).toEqual({ "Idempotency-Key": "idem-create" });
    expect(JSON.parse(calls[0]!.init.body as string)).not.toHaveProperty("input");
    expect(calls[1]!.path).toBe("/api/sessions/sess_1/messages");
    expect(calls[1]!.init.method).toBe("POST");
    expect(calls[1]!.init.headers).toEqual({ "Idempotency-Key": "idem-create:message" });
    expect(JSON.parse(calls[1]!.init.body as string)).toEqual({ input: "start now" });
  });

  it("createSessionWithMessage accepts an explicit first-run idempotency key", async () => {
    const { http, calls } = httpStub();
    await operations.createSessionWithMessage(
      http,
      {
        submission: {
          model: "deepseek/deepseek-v4-flash",
          assets: { files: [], skills: [], tools: [], instructions: [] },
          builtinTools: "default",
          mcpServers: []
        },
        secrets: {}
      },
      "start now",
      { idempotencyKey: "idem-create", messageIdempotencyKey: "idem-turn" }
    );

    expect(calls[1]!.init.headers).toEqual({ "Idempotency-Key": "idem-turn" });
  });

  it("derives a bounded deterministic first-message key from a 255-character create key", async () => {
    const { http, calls } = httpStub();
    const createKey = "k".repeat(255);
    await operations.createSessionWithMessage(
      http,
      {
        submission: {
          model: "deepseek/deepseek-v4-flash",
          assets: { files: [], skills: [], tools: [], instructions: [] },
          builtinTools: "default",
          mcpServers: []
        },
        secrets: {}
      },
      "start now",
      { idempotencyKey: createKey }
    );

    const messageKey = (calls[1]!.init.headers as Record<string, string>)["Idempotency-Key"]!;
    const derivedAgain = operations.deriveMessageIdempotencyKey(createKey);
    expect((calls[0]!.init.headers as Record<string, string>)["Idempotency-Key"]).toBe(createKey);
    expect(messageKey.length).toBeLessThanOrEqual(255);
    expect(messageKey).toBe(derivedAgain);
    expect(operations.deriveMessageIdempotencyKey("j".repeat(255))).not.toBe(derivedAgain);
    expect(messageKey).not.toBe(`${createKey}:message`);
  });

  it("rejects an oversized explicit first-message key before creating the session", async () => {
    const { http, calls } = httpStub();
    await expect(operations.createSessionWithMessage(
      http,
      {
        submission: {
          model: "deepseek/deepseek-v4-flash",
          assets: { files: [], skills: [], tools: [], instructions: [] },
          builtinTools: "default",
          mcpServers: []
        },
        secrets: {}
      },
      "start now",
      { idempotencyKey: "create", messageIdempotencyKey: "m".repeat(256) }
    )).rejects.toThrow(/at most 255 characters/);
    expect(calls).toHaveLength(0);
  });

  it("uses the session event ticket route", async () => {
    const { http, calls } = httpStub();
    const ticket = await operations.getSessionCoordinatorTicket(http, "sess_1");

    expect(ticket.ticket).toBe("ticket");
    expect(calls[0]!.path).toBe("/api/sessions/sess_1/events/ticket");
    expect(calls[0]!.init.method).toBe("POST");
  });

  it("lists session messages from the transcript route", async () => {
    const { http, calls } = httpStub();
    const page = await operations.listSessionMessages(http, "sess_1", {
      limit: 10,
      cursor: "cursor-1",
      since: "2026-07-02T00:00:00.000Z"
    });

    expect(calls[0]!.path).toBe("/api/sessions/sess_1/messages");
    expect(calls[0]!.init).toEqual({});
    expect(calls[0]!.query).toEqual({
      limit: "10",
      cursor: "cursor-1",
      since: "2026-07-02T00:00:00.000Z"
    });
    expect(page.messages[0]).toEqual({
      id: "msg_1",
      sender: "assistant",
      text: "hello",
      timestamp: "2026-07-02T12:00:00.000Z",
      turnSeq: 1,
      sequence: 1,
      messageId: "provider-msg-1"
    });
    expect(page.nextCursor).toBe("cursor-2");
  });

  it("posts session cancel without claiming idempotency", async () => {
    const { http, calls } = httpStub();
    await operations.cancelSession(http, "sess_1");

    expect(calls).toHaveLength(1);
    expect(calls[0]!.path).toBe("/api/sessions/sess_1/cancel");
    expect(calls[0]!.init.method).toBe("POST");
    expect(calls[0]!.init.headers).toBeUndefined();
  });

  it("types idleTtl as the idle-to-suspend timer", () => {
    const retention: SessionRetentionPolicy = {
      idleTtl: "3m"
    };

    expect(retention).toEqual({ idleTtl: "3m" });
  });
});
