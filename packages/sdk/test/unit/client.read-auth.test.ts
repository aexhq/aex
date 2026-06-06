/**
 * H-1 coherence: the run read + output-download surface on the hosted API
 * plane is now workspace-token gated. Our own clients
 * must keep working against it, which means EVERY read path has to send
 * `Authorization: Bearer <apiToken>`.
 *
 * The shared `HttpClient` attaches that header on both `request()` and
 * `download()`, so all `operations.*` reads inherit it. These tests pin
 * that invariant at the SDK boundary: a recording fetch captures the
 * Authorization header each read op sent. If a future refactor routes a
 * read around the token-bearing transport, one of these turns red BEFORE
 * it can break against the gated routes in production.
 */
import { describe, expect, it } from "vitest";
import { AgentExecutor } from "../../src/index.js";

const TOKEN = "apt_read_auth_token";
const BASE = "https://example.test";

interface RecordedCall {
  readonly url: string;
  readonly authorization: string | null;
}

/**
 * Build a client whose fetch records the URL + Authorization header of
 * every request and returns the supplied body. The recorded calls let
 * each test assert the Bearer rode along.
 */
function recordingClient(body: unknown, contentType = "application/json") {
  const calls: RecordedCall[] = [];
  const stub: typeof fetch = async (input, init) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const headers = new Headers(init?.headers);
    calls.push({ url, authorization: headers.get("authorization") });
    return new Response(typeof body === "string" ? body : JSON.stringify(body), {
      status: 200,
      headers: { "content-type": contentType }
    });
  };
  const client = new AgentExecutor({ apiToken: TOKEN, baseUrl: BASE, fetch: stub });
  return { client, calls };
}

describe("SDK read paths send the workspace token (H-1 coherence)", () => {
  it("getRun sends Authorization: Bearer", async () => {
    const { client, calls } = recordingClient({ id: "run-1", status: "succeeded" });
    await client.getRun("run-1");
    expect(calls).toHaveLength(1);
    expect(calls[0]!.url).toBe(`${BASE}/api/runs/run-1`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("listEvents sends Authorization: Bearer", async () => {
    const { client, calls } = recordingClient({ events: [] });
    await client.listEvents("run-1");
    expect(calls[0]!.url).toBe(`${BASE}/api/runs/run-1/events`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("listOutputs sends Authorization: Bearer", async () => {
    const { client, calls } = recordingClient({ outputs: [] });
    await client.listOutputs("run-1");
    expect(calls[0]!.url).toBe(`${BASE}/api/runs/run-1/outputs`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("createOutputLink sends Authorization: Bearer (POST /link)", async () => {
    const { client, calls } = recordingClient({ url: `${BASE}/api/runs/run-1/outputs/abc/download` });
    await client.createOutputLink("run-1", "abc");
    expect(calls[0]!.url).toBe(`${BASE}/api/runs/run-1/outputs/abc/link`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("downloadOutput by id sends Authorization: Bearer to the gated download route", async () => {
    const { client, calls } = recordingClient("hello", "text/plain");
    const bytes = await client.downloadOutput("run-1", { id: "abc" });
    expect(new TextDecoder().decode(bytes)).toBe("hello");
    expect(calls[0]!.url).toBe(`${BASE}/api/runs/run-1/outputs/abc/download`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("downloadOutput by path sends Authorization: Bearer on list and download", async () => {
    const calls: RecordedCall[] = [];
    const stub: typeof fetch = async (input, init) => {
      const url =
        typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      const headers = new Headers(init?.headers);
      calls.push({ url, authorization: headers.get("authorization") });
      if (url.endsWith("/api/runs/run-1/outputs")) {
        return new Response(JSON.stringify({ outputs: [{ id: "abc", filename: "reports/result.txt" }] }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      return new Response("hello", { status: 200, headers: { "content-type": "text/plain" } });
    };
    const client = new AgentExecutor({ apiToken: TOKEN, baseUrl: BASE, fetch: stub });

    const bytes = await client.downloadOutput("run-1", { path: "result.txt", match: "suffix" });

    expect(new TextDecoder().decode(bytes)).toBe("hello");
    expect(calls.map((c) => c.url)).toEqual([
      `${BASE}/api/runs/run-1/outputs`,
      `${BASE}/api/runs/run-1/outputs/abc/download`
    ]);
    expect(calls.every((c) => c.authorization === `Bearer ${TOKEN}`)).toBe(true);
  });

  it("downloadOutput without a selector downloads the outputs zip with Authorization: Bearer", async () => {
    const { client, calls } = recordingClient({ outputs: [] });
    const bytes = await client.downloadOutput("run-1");
    expect(bytes.byteLength).toBeGreaterThan(0);
    expect(calls[0]!.url).toBe(`${BASE}/api/runs/run-1/outputs`);
    expect(calls[0]!.authorization).toBe(`Bearer ${TOKEN}`);
  });

  it("download assembles the run zip and every read carries Authorization: Bearer", async () => {
    // `download` is the SDK's whole-run verb: it fans out to getRun +
    // listEvents + listOutputs (and per-output /download) and zips the result
    // client-side. EVERY one of those reads must carry the token because the
    // public read/download surface is gated.
    const { client, calls } = recordingClient({ events: [], outputs: [] });
    await client.download("run-1");
    expect(calls.map((c) => c.url)).toEqual([
      `${BASE}/api/runs/run-1`,
      `${BASE}/api/runs/run-1/events`,
      `${BASE}/api/runs/run-1/outputs`
    ]);
    expect(calls.every((c) => c.authorization === `Bearer ${TOKEN}`)).toBe(true);
  });
});
