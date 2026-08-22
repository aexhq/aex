import assert from "node:assert/strict";
import test from "node:test";

import { compileTools } from "@aexhq/brain";
import { Aex } from "@aexhq/sdk";
import {
  bash,
  edit,
  read,
  sandbox,
  storage,
  subagents,
  webFetch,
  webSearch,
  write,
} from "../dist/index.js";

test("official state tools expose one stable action-discriminated capability each", () => {
  assert.equal(subagents().kind, "brain.tool");
  assert.equal(subagents().name, "subagents");
  assert.equal(subagents().execution, "engine");
  assert.equal(subagents().executor.capability, "brain.subagents");
  assert.equal(storage().executor.capability, "brain.storage");
  assert.equal(sandbox().executor.capability, "brain.sandbox");
  assert.ok(storage().input.safeParse({ action: "list" }).success);
  assert.ok(sandbox().input.safeParse({ action: "create" }).success);
  assert.ok(!storage().input.safeParse({ action: "delete", key: "report" }).success);
  assert.ok(Object.isFrozen(subagents()));
});

test("hand helpers select individual builtins", () => {
  assert.deepEqual(
    [bash(), read(), write(), edit()].map((tool) => tool.name),
    ["bash", "read", "write", "edit"],
  );
});

test("the selected portable bundle retains its explicit runtime name", async () => {
  const compiled = await compileTools([bash()]);
  const loaded = await import(`data:text/javascript;base64,${compiled.bundles[0].content_base64}`);
  assert.equal(loaded.default.name, "bash");
  assert.equal(typeof loaded.default.execute, "function");
});

test("managed web helpers select only their matching builtins", () => {
  assert.equal(webSearch().name, "web_search");
  assert.equal(webFetch().name, "web_fetch");
  assert.equal(webSearch().executor.capability, "aex.web.search");
  assert.equal(webFetch().executor.capability, "aex.web.fetch");
  assert.equal(webSearch().execution, "engine");
  assert.equal(webFetch().execution, "engine");
});

test("SDK creation compiles imported values into Brain's sealed ordered Tool grant", async () => {
  let body;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (_input, init) => {
      body = JSON.parse(init.body);
      return Response.json({
        id: "ses_01",
        root_id: "ses_01",
        depth: 0,
        object: "session",
        state: "open",
        turn_state: "idle",
        model: { provider: "anthropic", name: "test", context_window_tokens: 32_768 },
        storage: { session_storage_bytes: 0, upload_reserved_bytes: 0 },
        created_at: "2026-08-19T10:00:00.000Z",
        updated_at: "2026-08-19T10:00:00.000Z",
        turns: 0,
        last_seq: 0,
        metadata: {},
      });
    },
  });

  await aex.sessions.create({
    model: { provider: "anthropic", name: "test", apiKey: "sk-ant-test" },
    tools: [bash(), read(), write(), edit(), storage(), sandbox(), subagents()],
  });

  assert.deepEqual(body.tools.items.map((item) => item.definition.name), [
    "bash",
    "read",
    "write",
    "edit",
    "storage",
    "sandbox",
    "subagents",
  ]);
  assert.deepEqual(body.tools.items.map((item) => item.executor.kind), [
    "aex_managed",
    "aex_managed",
    "aex_managed",
    "aex_managed",
    "engine",
    "engine",
    "engine",
  ]);
  assert.equal(body.tool_bundles.length, 4);
});
