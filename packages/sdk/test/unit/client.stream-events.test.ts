/**
 * SDK-level coverage for `AexClient.streamEvents` (the loose `RunEvent`
 * snapshot poll loop). It polls the coordinator-backed `/events` endpoint,
 * dedupes by event id, and stops on a terminal run status or an abort. The
 * low-latency live envelope stream is covered separately (streamEnvelopes →
 * coordinator WS, shared event-stream-client tests).
 */
import { describe, expect, it } from "vitest";
import { AexClient } from "../../src/index.js";

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

function makeFetch(plan: ReadonlyArray<{ match: RegExp; respond: () => Response }>): {
  fetch: typeof fetch;
  calls: string[];
} {
  const calls: string[] = [];
  const fakeFetch: typeof fetch = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    calls.push(url);
    for (const entry of plan) {
      if (entry.match.test(url)) return entry.respond();
    }
    throw new Error(`No fake responder for ${url}`);
  };
  return { fetch: fakeFetch, calls };
}

describe("AexClient.streamEvents — polling the coordinator-backed /events", () => {
  it("yields events, dedupes by id across polls, and stops on terminal status", async () => {
    let listCount = 0;
    let getCount = 0;
    const { fetch: f, calls } = makeFetch([
      {
        match: /\/events$/,
        respond: () => {
          listCount++;
          const byCall: Record<number, ReadonlyArray<{ id: string; type: string }>> = {
            1: [{ id: "e1", type: "TEXT_MESSAGE_CONTENT" }],
            2: [
              { id: "e1", type: "TEXT_MESSAGE_CONTENT" },
              { id: "e2", type: "TEXT_MESSAGE_CONTENT" }
            ]
          };
          return jsonResponse({ events: byCall[listCount] ?? [] });
        }
      },
      {
        match: /\/runs\/run-abc$/,
        respond: () => {
          getCount++;
          return jsonResponse({ id: "run-abc", status: getCount >= 2 ? "succeeded" : "running" });
        }
      }
    ]);

    const client = new AexClient({ apiToken: "tk", baseUrl: "https://dash.test", fetch: f });
    const events: string[] = [];
    for await (const ev of client.streamEvents("run-abc", { intervalMs: 1 })) {
      events.push(ev.id);
    }
    expect(events).toEqual(["e1", "e2"]);
    // No SSE endpoint is ever touched.
    expect(calls.some((u) => u.endsWith("/events/stream"))).toBe(false);
  });

  it("stops promptly when the signal aborts", async () => {
    const { fetch: f } = makeFetch([
      { match: /\/events$/, respond: () => jsonResponse({ events: [] }) },
      { match: /\/runs\/run-abc$/, respond: () => jsonResponse({ id: "run-abc", status: "running" }) }
    ]);
    const client = new AexClient({ apiToken: "tk", baseUrl: "https://dash.test", fetch: f });
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 5);
    const events: string[] = [];
    for await (const ev of client.streamEvents("run-abc", { signal: controller.signal, intervalMs: 1 })) {
      events.push(ev.id);
    }
    expect(events).toEqual([]);
  });
});
