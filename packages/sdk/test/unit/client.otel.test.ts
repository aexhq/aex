import { describe, expect, it } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex, SessionStateError } from "../../src/index.js";

const BASE_URL = "https://example.test";
const SESSION_ID = "ses_otel";

interface OtlpPage {
  readonly resourceSpans?: readonly unknown[];
  readonly resourceLogs?: readonly unknown[];
}

interface SessionOtelAccessor {
  traces(): AsyncIterable<OtlpPage>;
  logs(): AsyncIterable<OtlpPage>;
}

function otelOf(session: unknown): SessionOtelAccessor {
  const candidate = (session as { readonly otel?: unknown }).otel;
  expect(candidate, "SessionHandle must expose session.otel").toBeTypeOf("object");
  if (candidate === null || typeof candidate !== "object") throw new TypeError("session.otel is not implemented");
  const accessor = candidate as Partial<SessionOtelAccessor>;
  expect(accessor.traces).toBeTypeOf("function");
  expect(accessor.logs).toBeTypeOf("function");
  return accessor as SessionOtelAccessor;
}

function sessionResponse(): Response {
  return Response.json({ session: { id: SESSION_ID, status: "idle", acceptsMessages: true } });
}

describe("session.otel", () => {
  it("iterates standards-pure trace pages using only x-aex-next-cursor", async () => {
    const requests: string[] = [];
    const first = { resourceSpans: [{ scopeSpans: [{ spans: [{ name: "invoke_agent" }] }] }] };
    const second = { resourceSpans: [{ scopeSpans: [{ spans: [{ name: "execute_tool" }] }] }] };
    const fetch: FetchLike = async (input) => {
      const url = String(input);
      if (url === `${BASE_URL}/api/sessions/${SESSION_ID}`) return sessionResponse();
      requests.push(url);
      const cursor = new URL(url).searchParams.get("cursor");
      return Response.json(
        cursor === null ? first : second,
        cursor === null ? { headers: { "x-aex-next-cursor": "opaque page/2" } } : undefined
      );
    };
    const client = new Aex({ apiKey: "test-token", baseUrl: BASE_URL, fetch });
    const session = await client.sessions.open(SESSION_ID);

    const pages: OtlpPage[] = [];
    for await (const page of otelOf(session).traces()) pages.push(page);

    expect(pages).toEqual([first, second]);
    expect(pages.every((page) => !("nextCursor" in page))).toBe(true);
    expect(requests).toEqual([
      `${BASE_URL}/api/sessions/${SESSION_ID}/otel?signal=traces`,
      `${BASE_URL}/api/sessions/${SESSION_ID}/otel?signal=traces&cursor=opaque+page%2F2`
    ]);
  });

  it("uses the logs signal without wrapping the OTLP body", async () => {
    const requests: string[] = [];
    const body = { resourceLogs: [{ scopeLogs: [{ logRecords: [{ severityText: "INFO" }] }] }] };
    const fetch: FetchLike = async (input) => {
      const url = String(input);
      if (url === `${BASE_URL}/api/sessions/${SESSION_ID}`) return sessionResponse();
      requests.push(url);
      return Response.json(body);
    };
    const client = new Aex({ apiKey: "test-token", baseUrl: BASE_URL, fetch });
    const session = await client.sessions.open(SESSION_ID);

    const pages: OtlpPage[] = [];
    for await (const page of otelOf(session).logs()) pages.push(page);

    expect(pages).toEqual([body]);
    expect(requests).toEqual([`${BASE_URL}/api/sessions/${SESSION_ID}/otel?signal=logs`]);
  });

  it("rejects a repeated response-header cursor instead of looping forever", async () => {
    const fetch: FetchLike = async (input) => {
      const url = String(input);
      if (url === `${BASE_URL}/api/sessions/${SESSION_ID}`) return sessionResponse();
      return Response.json({ resourceSpans: [] }, { headers: { "x-aex-next-cursor": "same" } });
    };
    const client = new Aex({ apiKey: "test-token", baseUrl: BASE_URL, fetch });
    const session = await client.sessions.open(SESSION_ID);

    const consume = async () => {
      for await (const _page of otelOf(session).traces()) {
        // consume until the cursor guard fires
      }
    };
    await expect(consume()).rejects.toBeInstanceOf(SessionStateError);
  });
});
