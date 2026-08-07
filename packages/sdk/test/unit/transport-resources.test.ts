import { describe, expect, test } from "bun:test";

import {
  Aex,
  type AexTransport,
  type WireRequest,
  type WireResponse,
} from "../../src/index.js";

const KEY = `aex_wk_euw1_0100000000e008000000000001_0100000000e008000000000000_${"A".repeat(42)}A`;

class ScriptedTransport implements AexTransport {
  readonly requests: WireRequest[] = [];

  async execute<T>(request: WireRequest): Promise<WireResponse<T>> {
    this.requests.push(request);
    return { status: 200, headers: new Headers(), body: { id: "fixture" } as T };
  }
}

describe("resource routing", () => {
  test("maps resource methods to route ids, paths, headers, and canonical body bytes", async () => {
    const transport = new ScriptedTransport();
    const aex = new Aex({ apiKey: KEY, transport });

    await aex.sessions.create({ provider: "openai", model: "gpt-test" }, { idempotencyKey: "idk_1" });
    await aex.sessions.get("ses_1");
    await aex.workspaces.get("wsp_1");

    expect(transport.requests.map((request) => request.routeId)).toEqual([
      "session_create",
      "session_get",
      "workspace_get",
    ]);
    expect(transport.requests[0]?.path).toBe("/api/sessions");
    expect(transport.requests[0]?.headers.get("Idempotency-Key")).toBe("idk_1");
    expect(new TextDecoder().decode(transport.requests[0]?.body)).toBe(
      '{"model":"gpt-test","provider":"openai"}',
    );
    expect(transport.requests[1]?.path).toBe("/api/sessions/ses_1");
    expect(transport.requests[2]?.path).toBe("/api/workspaces/wsp_1");
  });

  test("rejects a model-only session selection before transport", async () => {
    const transport = new ScriptedTransport();
    const aex = new Aex({ apiKey: KEY, transport });
    await expect(
      aex.sessions.create({ model: "gpt-test" } as { provider: string; model: string }),
    ).rejects.toThrow("provider");
    expect(transport.requests).toHaveLength(0);
  });
});
