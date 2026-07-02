/**
 * H-1 coherence: the run read + output-download surface on the hosted API
 * plane is workspace-token gated. Our own clients must keep working against
 * it, which means EVERY read path has to send `Authorization: Bearer <apiToken>`.
 *
 * The shared `HttpClient` attaches that header on both `request()` and
 * `download()`, so all `operations.*` reads inherit it. These tests pin
 * that invariant at the SDK boundary via the `SessionHandle` read surface:
 * a recording fetch captures the Authorization header each read op sent. If a
 * future refactor routes a read around the token-bearing transport, one of
 * these turns red BEFORE it can break against the gated routes in production.
 */
import { describe, expect, it } from "vitest";
import { Aex, type SessionHandle } from "../../src/index.js";

const TOKEN = "apt_read_auth_token";
const BASE = "https://example.test";
const SID = "sess-1";

interface RecordedCall {
  readonly url: string;
  readonly authorization: string | null;
  readonly body?: string;
}

/**
 * Build a client whose fetch records the URL + Authorization header of every
 * request and returns the supplied body — EXCEPT the session-rehydrate read
 * (`GET /api/sessions/sess-1`, used by `openSession`) which always returns a
 * minimal session record so a handle can be built.
 */
function recordingClient(body: unknown, contentType = "application/json") {
  const calls: RecordedCall[] = [];
  const stub: typeof fetch = async (input, init) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = (init?.method ?? "GET").toString();
    const headers = new Headers(init?.headers);
    calls.push({
      url,
      authorization: headers.get("authorization"),
      ...(typeof init?.body === "string" ? { body: init.body } : {})
    });
    if (method === "GET" && url.endsWith(`/api/sessions/${SID}`)) {
      return new Response(JSON.stringify({ id: SID, status: "succeeded" }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    return new Response(typeof body === "string" ? body : JSON.stringify(body), {
      status: 200,
      headers: { "content-type": contentType }
    });
  };
  const client = new Aex({ apiToken: TOKEN, baseUrl: BASE, fetch: stub });
  return { client, calls };
}

/** Open the session handle, then drop the rehydrate read so tests assert only the op under test. */
async function openHandle(client: Aex, calls: RecordedCall[]): Promise<SessionHandle> {
  const session = await client.openSession(SID);
  calls.length = 0;
  return session;
}

describe("SDK read paths send the workspace token (H-1 coherence)", () => {
  it("refresh sends Authorization: Bearer", async () => {
    const { client, calls } = recordingClient({ id: SID, status: "succeeded" });
    const session = await openHandle(client, calls);
    await session.refresh();
    expect(calls).toHaveLength(1);
    expect(calls[0]!.url).toBe(`${BASE}/api/sessions/${SID}`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("listEvents sends Authorization: Bearer", async () => {
    const { client, calls } = recordingClient({ events: [] });
    const session = await openHandle(client, calls);
    await session.events().list();
    expect(calls[0]!.url).toBe(`${BASE}/api/sessions/${SID}/events`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("listOutputs sends Authorization: Bearer", async () => {
    const { client, calls } = recordingClient({ outputs: [] });
    const session = await openHandle(client, calls);
    await session.outputs().list();
    expect(calls[0]!.url).toBe(`${BASE}/api/sessions/${SID}/outputs`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("outputLink by id sends Authorization: Bearer (POST /link)", async () => {
    const { client, calls } = recordingClient({ url: `${BASE}/api/runs/${SID}/outputs/abc/download` });
    const session = await openHandle(client, calls);
    await session.outputs().link("abc");
    expect(calls[0]!.url).toBe(`${BASE}/api/runs/${SID}/outputs/abc/link`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("outputLink resolves queries with Authorization: Bearer and sends the TTL body", async () => {
    const calls: RecordedCall[] = [];
    const stub: typeof fetch = async (input, init) => {
      const url =
        typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      const method = (init?.method ?? "GET").toString();
      const headers = new Headers(init?.headers);
      calls.push({
        url,
        authorization: headers.get("authorization"),
        ...(typeof init?.body === "string" ? { body: init.body } : {})
      });
      if (method === "GET" && url.endsWith(`/api/sessions/${SID}`)) {
        return new Response(JSON.stringify({ id: SID, status: "succeeded" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      if (url.endsWith(`/api/runs/${SID}/outputs`)) {
        return new Response(JSON.stringify({ outputs: [{ id: "abc", filename: "reports/result.txt" }] }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      return new Response(JSON.stringify({ url: "https://objects.example/result.txt" }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    };
    const client = new Aex({ apiToken: TOKEN, baseUrl: BASE, fetch: stub });
    const session = await client.openSession(SID);
    calls.length = 0;

    const link = await session.outputs().link({ filename: "result.txt" }, { expiresIn: "15m" });

    expect(link).toMatchObject({
      url: "https://objects.example/result.txt",
      expiresInSeconds: 900,
      output: { id: "abc", filename: "reports/result.txt" }
    });
    expect(calls.map((c) => c.url)).toEqual([
      `${BASE}/api/runs/${SID}/outputs`,
      `${BASE}/api/runs/${SID}/outputs/abc/link`
    ]);
    expect(calls.every((c) => c.authorization === `Bearer ${TOKEN}`)).toBe(true);
    expect(JSON.parse(calls[1]!.body!)).toEqual({ expiresInSeconds: 900 });
  });

  it("fetchOutput fetches the temporary direct URL without the SDK Authorization header", async () => {
    const calls: RecordedCall[] = [];
    const directUrl = "https://objects.example/result.txt?X-Amz-Signature=abc";
    const stub: typeof fetch = async (input, init) => {
      const url =
        typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      const method = (init?.method ?? "GET").toString();
      const headers = new Headers(init?.headers);
      calls.push({ url, authorization: headers.get("authorization") });
      if (method === "GET" && url.endsWith(`/api/sessions/${SID}`)) {
        return new Response(JSON.stringify({ id: SID, status: "succeeded" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      if (url.endsWith(`/api/runs/${SID}/outputs`)) {
        return new Response(JSON.stringify({ outputs: [{ id: "abc", filename: "result.txt" }] }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      if (url.endsWith(`/api/runs/${SID}/outputs/abc/link`)) {
        return new Response(JSON.stringify({ url: directUrl }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      return new Response("direct-bytes", { status: 200, headers: { "content-type": "text/plain" } });
    };
    const client = new Aex({ apiToken: TOKEN, baseUrl: BASE, fetch: stub });
    const session = await client.openSession(SID);
    calls.length = 0;

    const response = await session.outputs().fetch({ filename: "result.txt" });

    expect(await response.text()).toBe("direct-bytes");
    expect(calls.map((c) => c.url)).toEqual([
      `${BASE}/api/runs/${SID}/outputs`,
      `${BASE}/api/runs/${SID}/outputs/abc/link`,
      directUrl
    ]);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
    expect(calls[1]!.authorization).toBe(`Bearer ${TOKEN}`);
    expect(calls[2]!.authorization).toBeNull();
  });

  it("eventArchiveLink sends Authorization: Bearer (POST /events/link)", async () => {
    const { client, calls } = recordingClient({ url: "https://objects.example/events.jsonl" });
    const session = await openHandle(client, calls);
    await session.events().archiveLink();
    expect(calls[0]!.url).toBe(`${BASE}/api/runs/${SID}/events/link`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("downloadOutput by id sends Authorization: Bearer to the gated download route", async () => {
    const { client, calls } = recordingClient("hello", "text/plain");
    const session = await openHandle(client, calls);
    const bytes = await session.outputs().download({ id: "abc" });
    expect(new TextDecoder().decode(bytes)).toBe("hello");
    expect(calls[0]!.url).toBe(`${BASE}/api/runs/${SID}/outputs/abc/download`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("downloadOutput by path sends Authorization: Bearer on list and download", async () => {
    const calls: RecordedCall[] = [];
    const stub: typeof fetch = async (input, init) => {
      const url =
        typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      const method = (init?.method ?? "GET").toString();
      const headers = new Headers(init?.headers);
      calls.push({ url, authorization: headers.get("authorization") });
      if (method === "GET" && url.endsWith(`/api/sessions/${SID}`)) {
        return new Response(JSON.stringify({ id: SID, status: "succeeded" }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      if (url.endsWith(`/api/runs/${SID}/outputs`)) {
        return new Response(JSON.stringify({ outputs: [{ id: "abc", filename: "reports/result.txt" }] }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      return new Response("hello", { status: 200, headers: { "content-type": "text/plain" } });
    };
    const client = new Aex({ apiToken: TOKEN, baseUrl: BASE, fetch: stub });
    const session = await client.openSession(SID);
    calls.length = 0;

    const bytes = await session.outputs().download({ path: "result.txt", match: "suffix" });

    expect(new TextDecoder().decode(bytes)).toBe("hello");
    expect(calls.map((c) => c.url)).toEqual([
      `${BASE}/api/runs/${SID}/outputs`,
      `${BASE}/api/runs/${SID}/outputs/abc/download`
    ]);
    expect(calls.every((c) => c.authorization === `Bearer ${TOKEN}`)).toBe(true);
  });

  it("downloadOutput without a selector downloads the outputs zip with Authorization: Bearer", async () => {
    const { client, calls } = recordingClient({ outputs: [] });
    const session = await openHandle(client, calls);
    const bytes = await session.outputs().download();
    expect(bytes.byteLength).toBeGreaterThan(0);
    expect(calls[0]!.url).toBe(`${BASE}/api/runs/${SID}/outputs`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("download assembles the run zip and every read carries Authorization: Bearer", async () => {
    // `download` is the SDK's whole-run verb: it fans out to getRun +
    // listRunEvents + listOutputs (and per-output /download) and zips the result
    // client-side. EVERY one of those reads must carry the token because the
    // public read/download surface is gated.
    const { client, calls } = recordingClient({ events: [], outputs: [] });
    const session = await openHandle(client, calls);
    await session.download();
    expect(calls.map((c) => c.url)).toEqual([
      `${BASE}/api/runs/${SID}`,
      `${BASE}/api/runs/${SID}/events`,
      `${BASE}/api/runs/${SID}/outputs`
    ]);
    expect(calls.every((c) => c.authorization === `Bearer ${TOKEN}`)).toBe(true);
  });
});
