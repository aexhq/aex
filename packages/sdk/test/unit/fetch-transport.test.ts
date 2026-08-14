import { describe, expect, test } from "bun:test";

import { FetchTransport } from "../../src/index.js";

describe("FetchTransport", () => {
  test("passes the exact request byte range as an ArrayBuffer-backed fetch body", async () => {
    const shared = new SharedArrayBuffer(5);
    const requestBody = new Uint8Array(shared, 1, 3);
    requestBody.set([7, 8, 9]);

    let receivedBody: BodyInit | null | undefined;
    const fetchLike = (async (_input: RequestInfo | URL, init?: RequestInit) => {
      receivedBody = init?.body;
      return new Response(null, { status: 204 });
    }) as typeof globalThis.fetch;
    const transport = new FetchTransport(
      { central: "https://central.example", regional: "https://regional.example" },
      fetchLike,
    );

    await transport.execute({
      routeId: "session_create",
      method: "POST",
      path: "/api/sessions",
      headers: new Headers(),
      body: requestBody,
    });

    expect(receivedBody).toBeInstanceOf(Uint8Array);
    const receivedBytes = receivedBody as Uint8Array;
    expect(receivedBytes.buffer).toBeInstanceOf(ArrayBuffer);
    expect(Array.from(receivedBytes)).toEqual([7, 8, 9]);
  });


  test("decodes NDJSON split across transport chunks without buffering the response", async () => {
    const chunks = [
      new TextEncoder().encode('{"sequence":"1","kind":"log"}\n{"sequence"'),
      new TextEncoder().encode(':"2","kind":"span"}\n'),
    ];
    const fetchLike = (async () => new Response(new ReadableStream<Uint8Array>({
      pull(controller) {
        const chunk = chunks.shift();
        if (chunk) controller.enqueue(chunk);
        else controller.close();
      },
    }), { status: 200, headers: { "content-type": "application/x-ndjson" } })) as unknown as typeof fetch;
    const transport = new FetchTransport(
      { central: "https://central.example", regional: "https://regional.example" },
      fetchLike,
    );
    const response = await transport.stream<{ sequence: string; kind: string }>({
      routeId: "session_telemetry_stream",
      method: "GET",
      path: "/api/sessions/ses_1/telemetry/stream",
      headers: new Headers(),
    });
    const frames = [];
    for await (const frame of response.frames ?? []) frames.push(frame);
    expect(frames).toEqual([
      { sequence: "1", kind: "log" },
      { sequence: "2", kind: "span" },
    ]);
  });
});
