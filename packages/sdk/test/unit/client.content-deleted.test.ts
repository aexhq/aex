/**
 * WS4 (metadata-only retention), SDK surface. After a session's
 * content-retention window elapses the platform tombstones it "metadata_only":
 * the session RECORD still reads (200), but every CONTENT endpoint answers HTTP
 * 410 `{ error: "content_deleted", sessionId, purgedAt, deletedBy }`.
 *
 * These pin the SDK contract:
 *   1. a 410 content_deleted from ANY content read throws `ContentDeletedError`
 *      carrying sessionId/purgedAt/deletedBy (mapped in the one wire→exception
 *      factory, so events/messages/files/manifest/download all inherit it);
 *   2. `session.events.stream()` on a purged session throws it too;
 *   3. `getSession` surfaces `dataState:"metadata_only"` + `contentPurgedAt`;
 *   4. a normal (active) session still parses with `dataState` absent — and an
 *      explicit `dataState:"active"` passes through unchanged.
 */
import { describe, expect, it } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex, AexApiError, ContentDeletedError, isContentDeleted, type SessionHandle } from "../../src/index.js";

const TOKEN = "aex_content_deleted_token";
const BASE = "https://example.test";
const SID = "sess-purged";
const PURGED_AT = "2026-07-17T12:00:00.000Z";

const CONTENT_DELETED_BODY = {
  error: "content_deleted",
  sessionId: SID,
  purgedAt: PURGED_AT,
  deletedBy: "retention"
} as const;

const ACTIVE_RECORD = { id: SID, status: "idle", acceptsMessages: true } as const;
const METADATA_ONLY_RECORD = {
  id: SID,
  status: "expired",
  acceptsMessages: false,
  dataState: "metadata_only",
  contentPurgedAt: PURGED_AT,
  contentDeletedBy: "retention"
} as const;

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

/**
 * A client whose session RECORD read returns `record` (200) but whose every
 * CONTENT endpoint returns the 410 content_deleted tombstone.
 */
function purgedClient(record: Record<string, unknown> = ACTIVE_RECORD): Aex {
  const stub: FetchLike = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = (init?.method ?? "GET").toUpperCase();
    const pathname = new URL(url).pathname;
    if (method === "GET" && pathname === `/api/sessions/${SID}`) {
      return json({ session: record });
    }
    return json(CONTENT_DELETED_BODY, 410);
  };
  return new Aex({ apiKey: TOKEN, baseUrl: BASE, fetch: stub });
}

/** A client whose GET session record returns exactly `record`. */
function recordClient(record: Record<string, unknown>): Aex {
  const stub: FetchLike = async () => json({ session: record });
  return new Aex({ apiKey: TOKEN, baseUrl: BASE, fetch: stub });
}

async function openHandle(client: Aex): Promise<SessionHandle> {
  return client.sessions.open(SID);
}

function expectContentDeleted(err: unknown): asserts err is ContentDeletedError {
  expect(err).toBeInstanceOf(ContentDeletedError);
  expect(err).toBeInstanceOf(AexApiError);
  expect(isContentDeleted(err)).toBe(true);
  const deleted = err as ContentDeletedError;
  expect(deleted.status).toBe(410);
  expect(deleted.apiCode).toBe("content_deleted");
  expect(deleted.sessionId).toBe(SID);
  expect(deleted.purgedAt).toBe(PURGED_AT);
  expect(deleted.deletedBy).toBe("retention");
}

async function captureRejected(operation: () => Promise<unknown>): Promise<unknown> {
  return operation().then(
    () => {
      throw new Error("expected the operation to reject");
    },
    (error: unknown) => error
  );
}

describe("WS4 metadata-only retention (SDK)", () => {
  it("throws ContentDeletedError when a content read (events.list) hits 410 content_deleted", async () => {
    const session = await openHandle(purgedClient());
    expectContentDeleted(await captureRejected(() => session.events.list()));
  });

  it("maps 410 on the files read too (any content fetch inherits the mapping)", async () => {
    const session = await openHandle(purgedClient());
    expectContentDeleted(await captureRejected(() => session.files.list()));
  });

  it("throws ContentDeletedError when .stream() is opened on a purged session", async () => {
    const session = await openHandle(purgedClient(METADATA_ONLY_RECORD));
    const drain = async (): Promise<void> => {
      // The polling stream reads getSession (200) then the events snapshot (410).
      for await (const _event of session.events.stream()) {
        // unreachable — the events read throws before yielding
      }
    };
    expectContentDeleted(await captureRejected(drain));
  });

  it("surfaces dataState:\"metadata_only\" + contentPurgedAt on getSession", async () => {
    const client = recordClient(METADATA_ONLY_RECORD);
    const session = await client.sessions.get(SID);
    expect(session.dataState).toBe("metadata_only");
    expect(session.contentPurgedAt).toBe(PURGED_AT);
    expect(session.contentDeletedBy).toBe("retention");
    // And SessionHandle.refresh()/record expose the same fields.
    const handle = await client.sessions.open(SID);
    const refreshed = await handle.refresh();
    expect(refreshed.dataState).toBe("metadata_only");
    expect(handle.record.contentPurgedAt).toBe(PURGED_AT);
  });

  it("parses an active session with dataState absent (backward compatible)", async () => {
    const session = await recordClient(ACTIVE_RECORD).sessions.get(SID);
    expect(session.dataState).toBeUndefined();
    expect(session.contentPurgedAt).toBeUndefined();
    expect(session.contentDeletedBy).toBeUndefined();
  });

  it("passes an explicit dataState:\"active\" through unchanged", async () => {
    const session = await recordClient({ ...ACTIVE_RECORD, dataState: "active" }).sessions.get(SID);
    expect(session.dataState).toBe("active");
    expect(session.contentPurgedAt).toBeUndefined();
  });

  it.each(["parent resolver", "parent events", "child events"] as const)(
    "preserves exact thrown-object identity for %s failures",
    async (failurePoint) => {
      const sentinel = new AexApiError(418, `sentinel ${failurePoint}`, { failurePoint });
      let sessionReads = 0;
      const stub: FetchLike = async (input) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
        const pathname = new URL(url).pathname;
        if (pathname === `/api/sessions/${SID}`) {
          sessionReads += 1;
          if (failurePoint === "parent resolver" && sessionReads === 2) throw sentinel;
          return json({ session: ACTIVE_RECORD });
        }
        if (pathname === `/api/sessions/${SID}/children`) {
          return json({
            children: [{
              id: "child-failure",
              parentSessionId: SID,
              status: "running",
              createdAt: "2026-07-21T00:00:00.000Z",
              updatedAt: "2026-07-21T00:00:01.000Z"
            }]
          });
        }
        if (pathname === `/api/sessions/${SID}/events` && failurePoint === "parent events") throw sentinel;
        if (pathname === "/api/sessions/child-failure/events" && failurePoint === "child events") throw sentinel;
        throw new Error(`unexpected request ${pathname}`);
      };
      const client = new Aex({ apiKey: TOKEN, baseUrl: BASE, fetch: stub, retry: false });
      const parent = await openHandle(client);
      const stream = failurePoint === "child events"
        ? (await parent.children())[0]!.events.stream()
        : parent.events.stream();
      const drain = async (): Promise<void> => {
        for await (const _event of stream) {
          // The selected failure occurs before the first yield.
        }
      };

      expect(await captureRejected(drain)).toBe(sentinel);
    }
  );
});
