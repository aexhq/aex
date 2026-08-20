import assert from "node:assert/strict";
import test from "node:test";

import { Aex } from "@aexhq/sdk";
import { bash, edit, read, subagents, webFetch, webSearch, write } from "../dist/index.js";

test("subagents selects the stable task primitive", () => {
  assert.equal(subagents().kind, "brain.tool");
  assert.equal(subagents().name, "task");
  assert.equal(subagents().execution, "intrinsic");
  assert.equal(subagents().executor.capability, "brain.subagents.v1");
  assert.ok(Object.isFrozen(subagents()));
});

test("hand helpers select individual builtins", () => {
  assert.deepEqual(
    [bash(), read(), write(), edit()].map((tool) => tool.name),
    ["bash", "read", "write", "edit"],
  );
});

test("managed web helpers select only their matching builtins", () => {
  assert.equal(webSearch().name, "web_search");
  assert.equal(webFetch().name, "web_fetch");
  assert.equal(webSearch().executor.capability, "aex.web.search.v1");
  assert.equal(webFetch().executor.capability, "aex.web.fetch.v1");
  assert.equal(webSearch().execution, "server");
  assert.equal(webFetch().execution, "server");
});

test("SDK creation compiles imported values into Brain's sealed ordered Tool grant", async () => {
  let body;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (_input, init) => {
      body = JSON.parse(init.body);
      return Response.json({
        id: "ses_01",
        object: "session",
        state: "idle",
        model: { provider: "anthropic", name: "test" },
        hand: { state: "ready", shape: "1gb" },
        storage: { workspace_bytes: 0, suspended_bytes: 0, artifact_bytes: 0 },
        created_at: "2026-08-19T10:00:00.000Z",
        updated_at: "2026-08-19T10:00:00.000Z",
        turns: 0,
        metadata: {},
      });
    },
  });

  await aex.sessions.create({
    model: { provider: "anthropic", name: "test", apiKey: "sk-ant-test" },
    tools: [bash(), read(), write(), edit(), subagents()],
  });

  assert.deepEqual(body.tools.items.map((item) => item.definition.name), [
    "bash",
    "read",
    "write",
    "edit",
    "task",
  ]);
  assert.deepEqual(body.tools.items.map((item) => item.executor.kind), [
    "hand",
    "hand",
    "hand",
    "hand",
    "intrinsic",
  ]);
  assert.equal(body.tool_bundles.length, 4);
});
