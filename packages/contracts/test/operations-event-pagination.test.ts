import { describe, expect, it } from "bun:test";
import {
  SessionConfigValidationError,
  SessionStateError,
  type HttpClient,
  type AexEvent
} from "../src/index.js";
import { operations } from "../src/internal.js";

function event(sequence: number): AexEvent {
  return {
    specversion: "1.0",
    id: `event-${sequence}`,
    source: "runtime",
    type: "CUSTOM",
    subject: "session-1",
    threadId: "session-1",
    runId: "run-1",
    time: new Date(sequence).toISOString(),
    sequence,
    data: { name: "page", value: sequence }
  };
}

const eventLists = [
  ["session events", operations.listSessionEvents]
] as const;

describe("event pagination", () => {
  it("iterates lazily, applies the requested page size, and stops fetching when the consumer breaks", async () => {
    const queries: Record<string, string>[] = [];
    const http = {
      async request<T>(_path: string, _init: RequestInit, query: Record<string, string>): Promise<T> {
        queries.push(query);
        return { events: [event(1)], nextCursor: "opaque-page-2" } as T;
      }
    } as unknown as HttpClient;

    const seen: AexEvent[] = [];
    for await (const value of operations.iterateSessionEvents(http, "session-1", { pageSize: 25 })) {
      seen.push(value);
      break;
    }

    expect(seen).toEqual([event(1)]);
    expect(queries).toEqual([{ limit: "25" }]);
  });

  it.each(eventLists)("%s transparently follows opaque string cursors", async (_name, list) => {
    const queries: Record<string, string>[] = [];
    const http = {
      async request<T>(_path: string, _init: RequestInit, query: Record<string, string>): Promise<T> {
        queries.push(query);
        return (query.cursor === undefined
          ? { events: [event(1)], nextCursor: "opaque-page-2" }
          : { events: [event(2)] }) as T;
      }
    } as unknown as HttpClient;

    await expect(list(http, "session-1")).resolves.toEqual([event(1), event(2)]);
    expect(queries).toEqual([{}, { cursor: "opaque-page-2" }]);
  });

  it("fails closed when an event page repeats a cursor", async () => {
    let requests = 0;
    const http = {
      async request<T>(): Promise<T> {
        requests += 1;
        return { events: [], nextCursor: "same-cursor" } as T;
      }
    } as unknown as HttpClient;

    await expect(operations.listSessionEvents(http, "session-1")).rejects.toBeInstanceOf(SessionStateError);
    expect(requests).toBe(2);
  });

  it("rejects obsolete numeric event cursors", async () => {
    const http = {
      async request<T>(): Promise<T> {
        return { events: [], nextCursor: 2 } as T;
      }
    } as unknown as HttpClient;

    await expect(operations.listSessionEvents(http, "session-1")).rejects.toBeInstanceOf(SessionStateError);
  });

  it.each([0, 1001, 1.5])("rejects an invalid iterator page size (%s)", async (pageSize) => {
    const http = {
      async request<T>(): Promise<T> {
        throw new Error("request must not be reached");
      }
    } as unknown as HttpClient;

    const read = async () => {
      for await (const _event of operations.iterateSessionEvents(http, "session-1", { pageSize })) {
        // The invalid option must fail before the transport is called.
      }
    };
    await expect(read()).rejects.toBeInstanceOf(SessionConfigValidationError);
  });
});
