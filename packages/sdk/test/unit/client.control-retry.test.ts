import { describe, expect, it } from "vitest";
import { Aex, type SessionHandle } from "../../src/index.js";

interface MutationCall {
  readonly method: string;
  readonly path: string;
  readonly idempotencyKey: string | null;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" }
  });
}

function controlHarness(): { readonly client: Aex; readonly calls: MutationCall[] } {
  const calls: MutationCall[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = new URL(
      typeof input === "string"
        ? input
        : input instanceof URL
          ? input.toString()
          : (input as Request).url
    );
    const method = String(init?.method ?? "GET").toUpperCase();
    if (method === "GET" && url.pathname === "/api/sessions/session-1") {
      return json({ session: { id: "session-1", status: "idle", acceptsMessages: true } });
    }
    calls.push({
      method,
      path: url.pathname,
      idempotencyKey: new Headers(init?.headers).get("Idempotency-Key")
    });
    return json({ error: "temporarily unavailable" }, 503);
  };

  return {
    client: new Aex({
      apiKey: "test-key",
      baseUrl: "https://api.test",
      fetch,
      retry: { initialDelayMs: 0, maxDelayMs: 0, maxAttempts: 4 }
    }),
    calls
  };
}

const handleControls: ReadonlyArray<{
  readonly name: string;
  readonly method: "POST" | "DELETE";
  readonly suffix: string;
  readonly invoke: (session: SessionHandle) => Promise<unknown>;
}> = [
  { name: "suspend", method: "POST", suffix: "/suspend", invoke: (session) => session.suspend() },
  { name: "cancel", method: "POST", suffix: "/cancel", invoke: (session) => session.cancel() },
  { name: "resume", method: "POST", suffix: "/resume", invoke: (session) => session.resume() },
  {
    name: "requestApproval",
    method: "POST",
    suffix: "/request-approval",
    invoke: (session) => session.requestApproval()
  },
  { name: "approve", method: "POST", suffix: "/approve", invoke: (session) => session.approve() },
  { name: "deny", method: "POST", suffix: "/deny", invoke: (session) => session.deny() },
  { name: "delete", method: "DELETE", suffix: "", invoke: (session) => session.delete() }
];

describe("lifecycle controls do not retry without server deduplication", () => {
  it.each(handleControls)("$name carries no idempotency key and gets one transport attempt", async (control) => {
    const { client, calls } = controlHarness();
    const session = await client.sessions.open("session-1");

    await expect(control.invoke(session)).rejects.toMatchObject({ status: 503 });

    expect(calls).toEqual([
      {
        method: control.method,
        path: `/api/sessions/session-1${control.suffix}`,
        idempotencyKey: null
      }
    ]);
  });

  it("sessions.delete carries no idempotency key and gets one transport attempt", async () => {
    const { client, calls } = controlHarness();

    await expect(client.sessions.delete("session-1")).rejects.toMatchObject({ status: 503 });

    expect(calls).toEqual([
      { method: "DELETE", path: "/api/sessions/session-1", idempotencyKey: null }
    ]);
  });
});
