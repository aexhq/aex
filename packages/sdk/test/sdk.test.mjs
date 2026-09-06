import assert from "node:assert/strict";
import test from "node:test";

import { agentloop, brainEnv, component } from "@aexhq/brain";
import { Aex } from "../dist/index.js";

test("uses the hosted credential with the neutral typed Brain API", async () => {
  const simple = agentloop({ implementation: component(new Uint8Array([1])) });
  const requests = [];
  const state = {
    session_id: "ses_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    status: "idle",
    last_sequence: 1,
  };
  const aex = new Aex({
    apiKey: "aex_sk_test",
    baseUrl: "https://api.example",
    fetch: async (input, init) => {
      const request = new Request(input, init);
      requests.push(request);
      if (request.url.endsWith("/v1/agentloops")) return Response.json({ id: "b".repeat(64), status: "admitted" });
      return Response.json(state);
    },
  });
  const session = await aex.sessions.create({
    model: { provider: "vercel-ai-gateway", name: "openai/gpt-5-mini", apiKey: "provider-key" },
    agentloop: simple({ env: brainEnv({ name: "brain" }) }),
  });
  await session.send("hello", { idempotencyKey: "message-one" });
  assert.equal(requests[0].headers.get("authorization"), "Bearer aex_sk_test");
  assert.equal(requests[2].headers.get("idempotency-key"), "message-one");
});

test("fails fast on an empty API key", () => {
  assert.throws(() => new Aex({ apiKey: "" }), /cannot be empty/u);
});
