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
});
