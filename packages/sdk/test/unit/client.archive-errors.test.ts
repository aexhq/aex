import { describe, expect, it } from "vitest";
import { Aex, AexApiError } from "../../src/index.js";

const BASE_URL = "https://example.test";
const SESSION_ID = "session-1";

function archiveErrorClient(status: 413 | 503, code: string) {
  let archiveAttempts = 0;
  const fetch: typeof globalThis.fetch = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (url === `${BASE_URL}/api/sessions/${SESSION_ID}`) {
      return Response.json({ session: { id: SESSION_ID, status: "idle", acceptsMessages: true } });
    }
    if (url === `${BASE_URL}/api/sessions/${SESSION_ID}/events/link`) {
      archiveAttempts += 1;
      return Response.json(
        { error: code, retryable: false, message: `archive failure: ${code}` },
        { status, headers: { "x-aex-retryable": "false" } }
      );
    }
    throw new Error(`unexpected request: ${url}`);
  };
  return {
    client: new Aex({ apiKey: "test-token", baseUrl: BASE_URL, fetch }),
    archiveAttempts: () => archiveAttempts
  };
}

describe("session.events.archiveLink errors", () => {
  it.each([
    { status: 413, code: "event_archive_too_large" },
    { status: 503, code: "event_archive_deadline_exceeded" }
  ] as const)("surfaces HTTP $status as $code without retrying", async ({ status, code }) => {
    const env = archiveErrorClient(status, code);
    const session = await env.client.sessions.open(SESSION_ID);

    const error = await session.events.archiveLink().catch((value: unknown) => value);

    expect(error).toBeInstanceOf(AexApiError);
    expect((error as AexApiError).status).toBe(status);
    expect((error as AexApiError).apiCode).toBe(code);
    expect(env.archiveAttempts()).toBe(1);
  });
});
