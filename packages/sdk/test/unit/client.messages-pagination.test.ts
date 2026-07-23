import { describe, expect, it } from "bun:test";
import { Aex, SessionStateError } from "../../src/index.js";

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" }
  });
}

function message(id: string, text: string) {
  return { id, sender: "assistant", text };
}

describe("session.messages.list pagination", () => {
  it("returns the full transcript across opaque cursors", async () => {
    const messageUrls: string[] = [];
    const client = new Aex({
      apiKey: "test-key",
      baseUrl: "https://api.test",
      retry: false,
      fetch: async (input) => {
        const url = new URL(
          typeof input === "string"
            ? input
            : input instanceof URL
              ? input.toString()
              : (input as Request).url
        );
        if (url.pathname === "/api/sessions/session-1") {
          return json({ session: { id: "session-1", status: "idle", acceptsMessages: true } });
        }
        messageUrls.push(url.toString());
        if (url.searchParams.get("cursor") === null) {
          return json({ messages: [message("m1", "one")], nextCursor: "page-2" });
        }
        return json({ messages: [message("m2", "two")] });
      }
    });

    const session = await client.sessions.open("session-1");
    await expect(session.messages.list()).resolves.toEqual([
      { id: "m1", sender: "assistant", text: "one" },
      { id: "m2", sender: "assistant", text: "two" }
    ]);
    expect(messageUrls).toEqual([
      "https://api.test/api/sessions/session-1/messages",
      "https://api.test/api/sessions/session-1/messages?cursor=page-2"
    ]);
  });

  it("fails closed when the server repeats a cursor", async () => {
    let messageRequests = 0;
    const client = new Aex({
      apiKey: "test-key",
      baseUrl: "https://api.test",
      retry: false,
      fetch: async (input) => {
        const url = new URL(
          typeof input === "string"
            ? input
            : input instanceof URL
              ? input.toString()
              : (input as Request).url
        );
        if (url.pathname === "/api/sessions/session-1") {
          return json({ session: { id: "session-1", status: "idle", acceptsMessages: true } });
        }
        messageRequests += 1;
        return json({ messages: [], nextCursor: "same-cursor" });
      }
    });

    const session = await client.sessions.open("session-1");
    await expect(session.messages.list()).rejects.toBeInstanceOf(SessionStateError);
    expect(messageRequests).toBe(2);
  });
});
