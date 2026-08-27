import assert from "node:assert/strict";
import test from "node:test";

import { defineAgentLoop } from "@aexhq/brain";
import { Aex } from "../dist/index.js";

test("uses the hosted credential with the neutral typed Brain API", async () => {
  const requests = [];
  const state = {
    session_id: "ses_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    journal_id: "jrn_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    status: "idle",
    through_sequence: 1,
    presentation_digest: "a".repeat(64),
  };
  const aex = new Aex({
    apiKey: "aex_sk_test",
    baseUrl: "https://api.example",
    fetch: async (input, init) => {
      const request = new Request(input, init);
      requests.push(request);
      if (request.url.endsWith("/v1/agentloops")) return Response.json({ digest: "b".repeat(64), status: "admitted" });
      return Response.json(state);
    },
  });
  const session = await aex.createSession({
    model: { provider: "vercel-ai-gateway", name: "openai/gpt-5-mini", apiKey: "provider-key" },
    agentLoop: defineAgentLoop(new Uint8Array([1])),
  });
  await session.send("hello", { idempotencyKey: "message-one" });
  assert.equal(requests[0].headers.get("authorization"), "Bearer aex_sk_test");
  assert.equal(requests[2].headers.get("idempotency-key"), "message-one");
});

test("fails fast on an empty API key", () => {
  assert.throws(() => new Aex({ apiKey: "" }), /cannot be empty/u);
});
