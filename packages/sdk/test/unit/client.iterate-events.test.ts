import { describe, expect, it } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import type { AexEvent } from "@aexhq/contracts";
import { Aex } from "../../src/index.js";

const BASE_URL = "https://example.test";
const SESSION_ID = "session-1";

function event(sequence: number): AexEvent {
  return {
    specversion: "1.0",
    id: `event-${sequence}`,
    source: "runtime",
    type: "CUSTOM",
    subject: SESSION_ID,
    threadId: SESSION_ID,
    runId: "run-1",
    time: new Date(sequence).toISOString(),
    sequence,
    data: { name: "page", value: sequence }
  };
}

function clientWithPagedEvents() {
  const eventRequests: string[] = [];
  const fetch: FetchLike = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (url === `${BASE_URL}/api/sessions/${SESSION_ID}`) {
      return Response.json({ session: { id: SESSION_ID, status: "idle", acceptsMessages: true } });
    }
    if (url.startsWith(`${BASE_URL}/api/sessions/${SESSION_ID}/events`)) {
      eventRequests.push(url);
      const cursor = new URL(url).searchParams.get("cursor");
      return Response.json(cursor === null
        ? { events: [event(1)], nextCursor: "page-2" }
        : { events: [event(2)] });
    }
    throw new Error(`unexpected request: ${url}`);
  };
  return { client: new Aex({ apiKey: "test-token", baseUrl: BASE_URL, fetch }), eventRequests };
}

describe("session.events.iterate", () => {
  it("yields all event pages without materializing them", async () => {
    const { client, eventRequests } = clientWithPagedEvents();
    const session = await client.sessions.open(SESSION_ID);

    const events = [];
    for await (const event of session.events.iterate({ pageSize: 25 })) events.push(event);

    expect(events.map((value) => value.sequence)).toEqual([1, 2]);
    expect(eventRequests).toEqual([
      `${BASE_URL}/api/sessions/${SESSION_ID}/events?limit=25`,
      `${BASE_URL}/api/sessions/${SESSION_ID}/events?limit=25&cursor=page-2`
    ]);
  });

  it("does not request another page after the consumer breaks", async () => {
    const { client, eventRequests } = clientWithPagedEvents();
    const session = await client.sessions.open(SESSION_ID);

    for await (const _event of session.events.iterate()) break;

    expect(eventRequests).toEqual([`${BASE_URL}/api/sessions/${SESSION_ID}/events`]);
  });

  it("keeps first() and last() memory-bounded", async () => {
    const firstEnv = clientWithPagedEvents();
    const firstSession = await firstEnv.client.sessions.open(SESSION_ID);
    await expect(firstSession.events.first()).resolves.toMatchObject({ sequence: 1 });
    expect(firstEnv.eventRequests).toHaveLength(1);

    const lastEnv = clientWithPagedEvents();
    const lastSession = await lastEnv.client.sessions.open(SESSION_ID);
    await expect(lastSession.events.last()).resolves.toMatchObject({ sequence: 2 });
    expect(lastEnv.eventRequests).toHaveLength(2);
  });
});
