import assert from "node:assert/strict";
import test from "node:test";

import { Aex } from "../dist/index.js";

test("uses the hosted base URL and API key through the neutral Brain API", async () => {
  const requests = [];
  const state = {
    session_id: "ses_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    journal_id: "jrn_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    status: "idle",
    through_sequence: 1,
    presentation_digest: "a".repeat(64),
    metadata: {},
  };
  const aex = new Aex({
    apiKey: "aex_sk_test",
    baseUrl: "https://api.example",
    fetch: async (input, init) => {
      const request = new Request(input, init);
      requests.push(request);
      if (request.url.endsWith("/v1/sessions") && request.method === "GET") return Response.json({ sessions: [state] });
      return Response.json(state);
    },
  });
  const sessions = await aex.sessions.list();
  assert.equal(sessions[0].id, state.session_id);
  await sessions[0].send("hello", { idempotencyKey: "message-one" });
  assert.equal(requests[0].headers.get("authorization"), "Bearer aex_sk_test");
  assert.equal(requests[2].headers.get("idempotency-key"), "message-one");
});

test("fails fast on an empty API key", () => {
  assert.throws(() => new Aex({ apiKey: "" }), /cannot be empty/u);
});
