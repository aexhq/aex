import { describe, expect, it } from "vitest";
import {
  SESSION_STATUSES,
  getRunStatusKind,
  operations,
  type HttpClient,
  type SessionRetentionPolicy,
  type SessionStatus
} from "../src/index.js";

function httpStub(): { readonly http: HttpClient; readonly calls: Array<{ readonly path: string; readonly init: RequestInit; readonly query: Record<string, string> }> } {
  const calls: Array<{ readonly path: string; readonly init: RequestInit; readonly query: Record<string, string> }> = [];
  const http = {
    async request<T>(path: string, init: RequestInit = {}, query: Record<string, string> = {}): Promise<T> {
      calls.push({ path, init, query });
      if (path === "/api/sessions") {
        return { session: { id: "sess_1", status: "idle" } } as T;
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
          session: { id: "sess_1", status: "running" },
          turn: { sessionId: "sess_1", turnSeq: 1 },
          eventCursor: 10
        } as T;
      }
      if (path.endsWith("/events/ticket")) {
        return { wsUrl: "wss://events.example.test/sess_1", ticket: "ticket", expiresAtMs: 1 } as T;
      }
      return { session: { id: "sess_1", status: "idle" } } as T;
    }
  } as HttpClient;
  return { http, calls };
}

describe("session contracts", () => {
  it("defines the resumable session status vocabulary separately from terminal runs", () => {
    const statuses = new Set<SessionStatus>(SESSION_STATUSES);
    expect(statuses.has("idle")).toBe(true);
    expect(statuses.has("suspended")).toBe(true);
    expect(statuses.has("cancelling")).toBe(true);
    expect(statuses.has("deleted")).toBe(true);
    expect(getRunStatusKind("idle")).toBe("active");
    expect(getRunStatusKind("suspended")).toBe("active");
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

  it("posts session cancel with an Idempotency-Key header", async () => {
    const { http, calls } = httpStub();
    await operations.cancelSession(http, "sess_1", { idempotencyKey: "idem-cancel" });

    expect(calls).toHaveLength(1);
    expect(calls[0]!.path).toBe("/api/sessions/sess_1/cancel");
    expect(calls[0]!.init.method).toBe("POST");
    expect(calls[0]!.init.headers).toEqual({ "Idempotency-Key": "idem-cancel" });
  });

  it("types idleTtl as the idle-to-suspend timer", () => {
    const retention: SessionRetentionPolicy = {
      idleTtl: "3m"
    };

    expect(retention).toEqual({ idleTtl: "3m" });
  });
});
