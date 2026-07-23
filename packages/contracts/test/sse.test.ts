import { describe, expect, it } from "bun:test";
import { SseParser, iterateSse } from "../src/sse.js";

describe("SseParser", () => {
  it("dispatches a single complete frame on blank-line terminator", () => {
    const parser = new SseParser();
    const frames = parser.pushText("event: ping\ndata: {}\n\n");
    expect(frames).toEqual([{ event: "ping", data: "{}" }]);
  });

  it("joins multiple data lines with \\n", () => {
    const parser = new SseParser();
    const frames = parser.pushText("event: x\ndata: line1\ndata: line2\ndata: line3\n\n");
    expect(frames).toEqual([{ event: "x", data: "line1\nline2\nline3" }]);
  });

  it("preserves event id across frames (last-event-id semantics)", () => {
    const parser = new SseParser();
    const frames = parser.pushText("id: cursor-a\nevent: ses_event\ndata: {}\n\nevent: ping\ndata: {}\n\n");
    expect(frames).toEqual([
      { id: "cursor-a", event: "ses_event", data: "{}" },
      // The spec: lastEventId PERSISTS across frames, so the second
      // ping frame still carries cursor-a as its id.
      { id: "cursor-a", event: "ping", data: "{}" }
    ]);
    expect(parser.lastEventId).toBe("cursor-a");
  });

  it("ignores comment lines", () => {
    const parser = new SseParser();
    const frames = parser.pushText(": keep-alive\nevent: ping\ndata: {}\n\n");
    expect(frames).toEqual([{ event: "ping", data: "{}" }]);
  });

  it("defaults event to 'message' when omitted", () => {
    const parser = new SseParser();
    const frames = parser.pushText("data: hello\n\n");
    expect(frames).toEqual([{ event: "message", data: "hello" }]);
  });

  it("strips leading space after the colon (but only one)", () => {
    const parser = new SseParser();
    const frames = parser.pushText("data:  two-leading-spaces\n\n");
    // First space stripped → " two-leading-spaces"
    expect(frames[0]?.data).toBe(" two-leading-spaces");
  });

  it("handles field with no value (no colon)", () => {
    const parser = new SseParser();
    const frames = parser.pushText("data\n\n");
    expect(frames).toEqual([{ event: "message", data: "" }]);
  });

  it("handles a frame split across many small chunks", () => {
    const parser = new SseParser();
    const chunks = "event: ses_event\nid: cur-1\ndata: {\"id\":\"x\"}\n\n".split("");
    const collected: ReturnType<SseParser["pushText"]>[number][] = [];
    for (const ch of chunks) {
      collected.push(...parser.pushText(ch));
    }
    expect(collected).toEqual([
      { id: "cur-1", event: "ses_event", data: '{"id":"x"}' }
    ]);
  });

  it("handles \\r\\n and \\r line terminators", () => {
    const parser = new SseParser();
    const crlf = parser.pushText("event: a\r\ndata: 1\r\n\r\n");
    expect(crlf).toEqual([{ event: "a", data: "1" }]);

    const parser2 = new SseParser();
    const cr = parser2.pushText("event: b\rdata: 2\r\r");
    expect(cr).toEqual([{ event: "b", data: "2" }]);
  });

  it("holds a trailing CR until the next chunk resolves a split CRLF", () => {
    const parser = new SseParser();
    expect(parser.pushText("data: hello\r")).toEqual([]);
    expect(parser.pushText("\ndata: world\r\n\r\n")).toEqual([
      { event: "message", data: "hello\nworld" }
    ]);
  });

  it("buffers a partial trailing frame until the next chunk", () => {
    const parser = new SseParser();
    const first = parser.pushText("event: x\ndata: par");
    expect(first).toEqual([]);
    const second = parser.pushText("tial\n\n");
    expect(second).toEqual([{ event: "x", data: "partial" }]);
  });

  it("accepts retry: as a non-negative integer", () => {
    const parser = new SseParser();
    const frames = parser.pushText("event: y\ndata: 1\nretry: 250\n\n");
    expect(frames[0]?.retry).toBe(250);
    const malformed = parser.pushText("event: z\ndata: 1\nretry: -1\n\n");
    expect(malformed[0]?.retry).toBeUndefined();
  });

  it("decodes a chunked UTF-8 multi-byte sequence", () => {
    // "café\n\n" → bytes: 63 61 66 c3 a9 0a 0a. Split mid c3a9 to ensure
    // the streaming TextDecoder reassembles correctly.
    const parser = new SseParser();
    const utf8 = new TextEncoder().encode("data: café\n\n");
    const split = utf8.findIndex((b) => b === 0xc3);
    expect(split).toBeGreaterThan(0);
    const first = parser.pushBytes(utf8.subarray(0, split + 1)); // half of é
    expect(first).toEqual([]);
    const second = parser.pushBytes(utf8.subarray(split + 1));
    expect(second).toEqual([{ event: "message", data: "café" }]);
  });
});

describe("iterateSse", () => {
  it("yields frames in order until the stream closes", async () => {
    const stream = makeReadableFromChunks([
      "event: a\ndata: 1\n\n",
      "event: b\ndata: 2\n\n",
      "event: c\ndata: 3\n\n"
    ]);
    const collected: string[] = [];
    for await (const frame of iterateSse(stream)) {
      collected.push(`${frame.event}:${frame.data}`);
    }
    expect(collected).toEqual(["a:1", "b:2", "c:3"]);
  });

  it("stops when the AbortSignal fires", async () => {
    const controller = new AbortController();
    const stream = makeReadableFromChunks([
      "event: a\ndata: 1\n\n",
      "event: b\ndata: 2\n\n"
    ], 5);
    const collected: string[] = [];
    for await (const frame of iterateSse(stream, controller.signal)) {
      collected.push(frame.event);
      controller.abort();
    }
    expect(collected).toEqual(["a"]);
  });

  it("releases the reader when the consumer breaks out early", async () => {
    let cancelled = false;
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        controller.enqueue(new TextEncoder().encode("event: x\ndata: 1\n\n"));
      },
      cancel() {
        cancelled = true;
      }
    });
    for await (const frame of iterateSse(stream)) {
      expect(frame.event).toBe("x");
      break;
    }
    // The `finally` block in the iterator must cancel the reader.
    expect(cancelled).toBe(true);
  });
});

function makeReadableFromChunks(chunks: readonly string[], delayMs = 0): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  let idx = 0;
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (idx >= chunks.length) {
        controller.close();
        return;
      }
      if (delayMs > 0) await new Promise((r) => setTimeout(r, delayMs));
      controller.enqueue(encoder.encode(chunks[idx]!));
      idx++;
    }
  });
}
