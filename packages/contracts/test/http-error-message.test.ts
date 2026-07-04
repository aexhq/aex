/**
 * AexApiError message extraction.
 *
 * Pins the `session_busy` translation: the 409 body carries the session's
 * CURRENT status (api contract), and the SDK surfaces it in the error message
 * so a send to a deleted session doesn't read as merely "busy".
 */
import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/http.js";
import { AexApiError } from "../src/sdk-errors.js";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function clientReturning(body: unknown, status: number): HttpClient {
  return new HttpClient({
    baseUrl: "https://api.example.test",
    apiToken: "t",
    fetch: async () => jsonResponse(body, status)
  });
}

describe("AexApiError message extraction", () => {
  it("session_busy carries the session's current status", async () => {
    const client = clientReturning(
      { error: "session_busy", sessionId: "run_x", runId: "run_x", status: "deleted", turnSeq: 0 },
      409
    );
    const err = await client.request("/api/sessions/run_x/messages", { method: "POST" }).then(
      () => null,
      (e: unknown) => e
    );
    expect(err).toBeInstanceOf(AexApiError);
    expect((err as AexApiError).message).toBe("session_busy (session status: deleted)");
    expect((err as AexApiError).status).toBe(409);
  });

  it("session_busy without a status field stays the bare code", async () => {
    const client = clientReturning({ error: "session_busy" }, 409);
    await expect(client.request("/api/x")).rejects.toThrow(/^session_busy$/);
  });

  it("other error codes are unchanged even when a status field exists", async () => {
    const client = clientReturning({ error: "not_found", status: "whatever" }, 404);
    await expect(client.request("/api/x")).rejects.toThrow(/^not_found$/);
  });

  it("surfaces the server's message detail alongside the error code", async () => {
    const client = clientReturning(
      {
        error: "asset_snapshot_source_missing",
        message: "referenced asset asset_xyz is not in the workspace store; upload and finalize it before referencing it",
        requestId: "req-1"
      },
      400
    );
    await expect(client.request("/api/sessions", { method: "POST" })).rejects.toThrow(
      "asset_snapshot_source_missing: referenced asset asset_xyz is not in the workspace store; upload and finalize it before referencing it"
    );
  });

  it("does not duplicate the code when message repeats it", async () => {
    const client = clientReturning({ error: "not_found", message: "not_found" }, 404);
    await expect(client.request("/api/x")).rejects.toThrow(/^not_found$/);
  });
});
