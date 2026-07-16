/** `operations.listSessions` validates the public page contract without masking server defects. */
import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/http.js";
import { operations } from "../src/internal.js";

const BASE = "https://api.test";

function clientFor(body: unknown, capture?: { url?: string }) {
  const fetchImpl = async (input: string | URL | Request) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    if (capture) capture.url = url;
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  };
  return new HttpClient({ apiKey: "tok", baseUrl: BASE, fetch: fetchImpl });
}

const WELL_FORMED = {
  id: "ses_a1",
  status: "idle",
  acceptsMessages: true,
  runtimeSize: "shared-1x-6gb",
  createdAt: "2026-07-04T04:21:51.650Z",
  updatedAt: "2026-07-04T04:22:44.525Z",
  costUsd: 0.0004
};

describe("operations.listSessions", () => {
  it("fails closed on rows missing required SessionSummary fields", async () => {
    const client = clientFor({
      sessions: [
        { id: "ses_a1", createdAt: "2026-07-04T04:22:44.525Z" },
        WELL_FORMED
      ],
      nextCursor: "opaque-cursor"
    });

    await expect(operations.listSessions(client, { limit: 5 })).rejects.toThrow(/row 0 has an invalid status/);
  });

  it("fails closed on costUsd:null instead of silently changing the response", async () => {
    const client = clientFor({
      sessions: [
        { ...WELL_FORMED, id: "ses_null", costUsd: null },
        WELL_FORMED
      ]
    });

    await expect(operations.listSessions(client)).rejects.toThrow(/invalid costUsd/);
  });

  it("fails closed on a run outcome used as a session status", async () => {
    const client = clientFor({ sessions: [{ ...WELL_FORMED, status: "succeeded" }] });
    await expect(operations.listSessions(client)).rejects.toThrow(/unknown lifecycle status/);
  });

  it.each(["sessionId", "runtime", "turnSeq", "cleanupStatus"])(
    "fails closed on the removed top-level %s field",
    async (field) => {
      const client = clientFor({ sessions: [{ ...WELL_FORMED, [field]: field === "turnSeq" ? 1 : "removed" }] });
      await expect(operations.listSessions(client)).rejects.toThrow(new RegExp(`removed ${field} field`));
    }
  );

  it("returns a clean page unchanged", async () => {
    const capture: { url?: string } = {};
    const client = clientFor({ sessions: [WELL_FORMED] }, capture);

    const page = await operations.listSessions(client, { limit: 3, cursor: "c1" });

    expect(page.sessions).toEqual([{
      id: WELL_FORMED.id,
      status: WELL_FORMED.status,
      acceptsMessages: true,
      runtime: { size: "shared-1x-6gb" },
      createdAt: WELL_FORMED.createdAt,
      updatedAt: WELL_FORMED.updatedAt,
      costUsd: WELL_FORMED.costUsd
    }]);
    expect(page.nextCursor).toBeUndefined();
    expect(capture.url).toBe(`${BASE}/api/sessions?limit=3&cursor=c1`);
  });

  it("rejects a legacy internal runtime token instead of exposing it publicly", async () => {
    const client = clientFor({ sessions: [{ ...WELL_FORMED, runtimeSize: "standard" }] });
    await expect(operations.listSessions(client)).rejects.toThrow(/invalid runtime/);
  });

  it("validates the page limit before transport", async () => {
    const client = clientFor({ sessions: [] });
    await expect(operations.listSessions(client, { limit: 0 })).rejects.toThrow(/between 1 and 100/);
    await expect(operations.listSessions(client, { limit: 101 })).rejects.toThrow(/between 1 and 100/);
    await expect(operations.listSessions(client, { limit: 1.5 })).rejects.toThrow(/between 1 and 100/);
  });

  it("validates lifecycle status and timestamp filters before transport", async () => {
    const client = clientFor({ sessions: [] });
    await expect(operations.listSessions(client, { status: "succeeded" as never }))
      .rejects.toThrow(/lifecycle status/);
    await expect(operations.listSessions(client, { since: "yesterday" }))
      .rejects.toThrow(/ISO-8601/);
  });
});
