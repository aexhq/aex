import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";

interface RecordedCall {
  readonly url: string;
  readonly method: string;
}

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" }
  });
}

function aliasClient(): { readonly client: Aex; readonly calls: RecordedCall[] } {
  const calls: RecordedCall[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = (init?.method ?? "GET").toString();
    calls.push({ url, method });
    if (url.endsWith("/api/sessions/sess-1/events")) return json({ events: [{ id: "evt-1", type: "agent.message" }] });
    if (url.endsWith("/api/sessions/sess-1/outputs")) return json({ outputs: [] });
    if (url.endsWith("/api/sessions/sess-1/cancel")) return json({ session: { id: "sess-1", status: "cancelling" } });
    if (url.endsWith("/api/sessions/sess-1")) {
      if (method === "DELETE") return new Response(null, { status: 204 });
      return json({ id: "sess-1", status: "idle" });
    }
    if (url.endsWith("/api/runs/sess-1/events")) return json({ events: [{ id: "evt-1", type: "agent.message" }] });
    if (url.endsWith("/api/runs/sess-1")) return json({ id: "sess-1", status: "succeeded" });
    return json({ id: "sess-1", status: "idle" });
  };
  return {
    client: new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch }),
    calls
  };
}

describe("SessionHandle operations delegate to the session/run endpoints", () => {
  it("route refresh/unit/listEvents/listOutputs/cancel/delete to the right verbs and paths", async () => {
    const { client, calls } = aliasClient();
    const session = await client.openSession("sess-1");
    // Drop the openSession rehydrate read; assert only the operations below.
    calls.length = 0;

    await session.refresh();
    await session.unit();
    await session.events().list();
    await session.outputs().list();
    await session.cancel();
    await session.delete();

    expect(calls).toEqual([
      { url: "https://example.test/api/sessions/sess-1", method: "GET" },
      { url: "https://example.test/api/runs/sess-1", method: "GET" },
      { url: "https://example.test/api/sessions/sess-1/events", method: "GET" },
      { url: "https://example.test/api/sessions/sess-1/outputs", method: "GET" },
      { url: "https://example.test/api/sessions/sess-1/cancel", method: "POST" },
      { url: "https://example.test/api/sessions/sess-1", method: "DELETE" }
    ]);
  });

  it("streamEvents polls the run events endpoint and stops once the session parks", async () => {
    const { client } = aliasClient();
    const session = await client.openSession("sess-1");
    const streamed: string[] = [];
    for await (const event of session.events().stream({ intervalMs: 1 })) {
      streamed.push(event.id);
    }
    expect(streamed).toEqual(["evt-1"]);
  });
});

describe("SessionHandle.messages / lastMessage decode assistant text", () => {
  function textClient(events: readonly unknown[]): Aex {
    const fetch: typeof globalThis.fetch = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      if (url.endsWith("/api/sessions/sess-1/events")) return json({ events });
      return json({ id: "sess-1", status: "idle" });
    };
    return new Aex({ apiKey: "tkn", baseUrl: "https://example.test", fetch });
  }

  it("returns assistant messages oldest-first and lastMessage is the latest", async () => {
    const client = textClient([
      { id: "e1", type: "TEXT_MESSAGE_CONTENT", data: { text: "Hello " } },
      { id: "e2", type: "TEXT_MESSAGE_CONTENT", data: { text: "world" } }
    ]);
    const session = await client.openSession("sess-1");
    expect((await session.messages().list()).map((m) => m.text)).toEqual(["Hello ", "world"]);
    expect((await session.messages().last())?.text).toBe("world");
  });

  it("lastMessage is undefined when the session produced no assistant text", async () => {
    const client = textClient([{ id: "e1", type: "RUN_STARTED", data: {} }]);
    const session = await client.openSession("sess-1");
    expect((await session.messages().last())?.text).toBeUndefined();
  });
});
